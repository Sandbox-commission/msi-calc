use csv::ReaderBuilder;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossterm::{cursor, execute, terminal};
use log::info;

use crate::checkpoint;
use crate::config::{sample_name, Config, VERSION};
use crate::tui::{fmt_duration, DisplayThread, ProgressState};

// ─── Data types ─────────────────────────────────────────────────────────────

pub(crate) struct SamplePair {
    pub tumor_bam: PathBuf,
    pub normal_bam: Option<PathBuf>,
}

// ─── Log helpers ────────────────────────────────────────────────────────────

fn make_log_stdio(log_dir: &Path, name: &str) -> (Stdio, Stdio) {
    let log_path = log_dir.join(format!("{name}.log"));
    match File::create(&log_path) {
        Ok(f) => {
            let f2 = f.try_clone().unwrap();
            (Stdio::from(f), Stdio::from(f2))
        }
        Err(_) => (Stdio::null(), Stdio::null()),
    }
}

// ─── Partial output cleanup ─────────────────────────────────────────────────

fn cleanup_partial_output(dir: &Path, name: &str) {
    let suffixes = ["", "_dis", "_germline", "_somatic", "_all", "_unstable"];
    for suffix in &suffixes {
        let path = dir.join(format!("{name}{suffix}"));
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
    }
    let msi_path = dir.join(format!("{name}_msisensor"));
    if msi_path.exists() {
        let _ = fs::remove_file(&msi_path);
    }
}

// ─── Run command with cancellation support ──────────────────────────────────

fn run_cancellable(mut cmd: Command) -> Result<bool, String> {
    let mut child = cmd.spawn().map_err(|e| format!("Failed to launch: {e}"))?;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) => {
                if crate::is_cancelled() {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Cancelled".to_string());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("Wait error: {e}")),
        }
    }
}

// ─── Phase 1: Profile normals ───────────────────────────────────────────────

fn profile_normal(
    normal_bam: &Path,
    baseline_dir: &Path,
    log_dir: &Path,
    output_dir: &Path,
    threads: u32,
    reference_list: &Path,
    state: &ProgressState,
    slot: usize,
) -> Result<PathBuf, String> {
    if crate::is_cancelled() {
        return Err("Cancelled".to_string());
    }

    let name = sample_name(normal_bam);
    let output_path = baseline_dir.join(&name);
    let all_file = PathBuf::from(format!("{}_all", output_path.display()));

    if !normal_bam.exists() {
        state.failed.fetch_add(1, Ordering::Relaxed);
        let msg = format!("Normal BAM not found: {}", normal_bam.display());
        state.add_event(format!("  FAIL  {} - {}", name, msg));
        return Err(msg);
    }

    state.set_active(slot, &format!("{name} (profiling)"));
    let job_start = Instant::now();

    let (stdout_cfg, stderr_cfg) = make_log_stdio(log_dir, &format!("{name}_profile"));
    let threads_str = threads.to_string();
    let output_str = output_path.to_string_lossy().to_string();

    let mut cmd = Command::new("msisensor-pro");
    cmd.args([
        "pro",
        "-d",
        &reference_list.to_string_lossy(),
        "-c",
        "20",
        "-t",
        &normal_bam.to_string_lossy(),
        "-b",
        &threads_str,
        "-o",
        &output_str,
    ])
    .stdout(stdout_cfg)
    .stderr(stderr_cfg);

    let result = run_cancellable(cmd);
    let dur = job_start.elapsed().as_secs_f64();

    match result {
        Ok(true) => {
            checkpoint::write_profile_checkpoint(output_dir, &name);
            state.record_duration(dur);
            state.completed.fetch_add(1, Ordering::Relaxed);
            state.add_event(format!("  DONE  {name} ({:.0}s)", dur));
            Ok(all_file)
        }
        Ok(false) => {
            cleanup_partial_output(baseline_dir, &name);
            checkpoint::remove_profile_checkpoint(output_dir, &name);
            state.failed.fetch_add(1, Ordering::Relaxed);
            state.add_event(format!("  FAIL  {name} (partial output cleaned)"));
            Err(format!("[{name}] msisensor-pro pro exited with error"))
        }
        Err(e) => {
            cleanup_partial_output(baseline_dir, &name);
            checkpoint::remove_profile_checkpoint(output_dir, &name);
            if e == "Cancelled" {
                state.add_event(format!("  STOP  {name} (cancelled, cleaned up)"));
            } else {
                state.failed.fetch_add(1, Ordering::Relaxed);
                state.add_event(format!("  FAIL  {name} - {e}"));
            }
            Err(e)
        }
    }
}

// ─── Phase 3: Process samples ───────────────────────────────────────────────

fn process_sample(
    output_dir: &Path,
    log_dir: &Path,
    threads: u32,
    pair: &SamplePair,
    baseline_file: Option<&Path>,
    reference_list: &Path,
    state: &ProgressState,
    slot: usize,
) -> Result<(), String> {
    if crate::is_cancelled() {
        return Err("Cancelled".to_string());
    }

    let sample = sample_name(&pair.tumor_bam);

    if !pair.tumor_bam.exists() {
        state.failed.fetch_add(1, Ordering::Relaxed);
        let msg = format!("Tumor BAM not found: {}", pair.tumor_bam.display());
        state.add_event(format!("  FAIL  {sample} - {msg}"));
        return Err(msg);
    }

    let is_paired = pair.normal_bam.is_some();
    let mode_label = if is_paired { "msi" } else { "pro" };
    state.set_active(slot, &format!("{sample} ({mode_label})"));
    let job_start = Instant::now();

    let (stdout_cfg, stderr_cfg) = make_log_stdio(log_dir, &sample);
    let output_path = output_dir.join(&sample);
    let output_str = output_path.to_string_lossy().to_string();
    let threads_str = threads.to_string();

    let mut cmd = Command::new("msisensor-pro");

    if let Some(ref normal) = pair.normal_bam {
        if !normal.exists() {
            state.failed.fetch_add(1, Ordering::Relaxed);
            let msg = format!("Normal BAM not found: {}", normal.display());
            state.add_event(format!("  FAIL  {sample} - {msg}"));
            return Err(msg);
        }
        cmd.args([
            "msi",
            "-d",
            &reference_list.to_string_lossy(),
            "-c",
            "20",
            "-n",
            &normal.to_string_lossy(),
            "-t",
            &pair.tumor_bam.to_string_lossy(),
            "-b",
            &threads_str,
            "-o",
            &output_str,
        ]);
    } else {
        let d_file = match baseline_file {
            Some(bf) => bf.to_string_lossy().to_string(),
            None => reference_list.to_string_lossy().to_string(),
        };
        cmd.args([
            "pro",
            "-d",
            &d_file,
            "-c",
            "20",
            "-t",
            &pair.tumor_bam.to_string_lossy(),
            "-b",
            &threads_str,
            "-o",
            &output_str,
        ]);
    }

    cmd.stdout(stdout_cfg).stderr(stderr_cfg);

    let result = run_cancellable(cmd);
    let dur = job_start.elapsed().as_secs_f64();

    match result {
        Ok(true) => {
            checkpoint::write_checkpoint(output_dir, &sample);
            state.record_duration(dur);
            state.completed.fetch_add(1, Ordering::Relaxed);
            state.add_event(format!("  DONE  {sample} ({:.0}s)", dur));
            Ok(())
        }
        Ok(false) => {
            cleanup_partial_output(output_dir, &sample);
            checkpoint::remove_checkpoint(output_dir, &sample);
            state.failed.fetch_add(1, Ordering::Relaxed);
            state.add_event(format!("  FAIL  {sample} (partial output cleaned)"));
            Err(format!("[{sample}] msisensor-pro exited with error"))
        }
        Err(e) => {
            cleanup_partial_output(output_dir, &sample);
            checkpoint::remove_checkpoint(output_dir, &sample);
            if e == "Cancelled" {
                state.add_event(format!("  STOP  {sample} (cancelled, cleaned up)"));
            } else {
                state.failed.fetch_add(1, Ordering::Relaxed);
                state.add_event(format!("  FAIL  {sample} - {e}"));
            }
            Err(e)
        }
    }
}

// ─── Work queue runner ──────────────────────────────────────────────────────

fn run_work_queue<T, F>(
    items: &[T],
    parallel_jobs: usize,
    state: &Arc<ProgressState>,
    worker: F,
) -> Vec<String>
where
    T: Sync,
    F: Fn(&T, usize) -> Result<(), String> + Sync,
{
    let next_idx = AtomicUsize::new(0);
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());

    std::thread::scope(|s| {
        for slot in 0..parallel_jobs {
            let next = &next_idx;
            let errs = &errors;
            let w = &worker;
            let st = &state;
            s.spawn(move || {
                loop {
                    if crate::is_cancelled() {
                        break;
                    }
                    let idx = next.fetch_add(1, Ordering::Relaxed);
                    if idx >= items.len() {
                        break;
                    }
                    if let Err(e) = w(&items[idx], slot) {
                        if e != "Cancelled" {
                            errs.lock().unwrap().push(e);
                        }
                    }
                    st.clear_slot(slot);
                    std::thread::yield_now();
                }
            });
        }
    });

    errors.into_inner().unwrap()
}

// ─── Main pipeline ──────────────────────────────────────────────────────────

pub(crate) fn run(config: Config, pairs_file: PathBuf) -> ExitCode {
    fs::create_dir_all(&config.output_dir).expect("Failed to create output directory");
    let log_dir = config.output_dir.join("logs");
    fs::create_dir_all(&log_dir).expect("Failed to create logs directory");
    fs::create_dir_all(checkpoint::checkpoint_dir(&config.output_dir))
        .expect("Failed to create checkpoints directory");

    // ── Phase 0: Scan reference to generate microsatellites.list ──
    if !config.reference_fasta.exists() {
        log::error!(
            "Reference FASTA not found: {}",
            config.reference_fasta.display()
        );
        return ExitCode::FAILURE;
    }
    let reference_list = config.output_dir.join("microsatellites.list");
    if reference_list.exists() {
        info!("Scan:   microsatellites.list already present, skipping scan.");
        info!("        {}", reference_list.display());
    } else {
        info!("Scan:   generating microsatellites.list from reference FASTA...");
        info!(
            "        msisensor-pro scan -d {} -o {}",
            config.reference_fasta.display(),
            reference_list.display()
        );
        let (scan_stdout, scan_stderr) = make_log_stdio(&log_dir, "scan");
        let mut scan_cmd = Command::new("msisensor-pro");
        scan_cmd
            .args([
                "scan",
                "-d",
                &config.reference_fasta.to_string_lossy(),
                "-o",
                &reference_list.to_string_lossy(),
            ])
            .stdout(scan_stdout)
            .stderr(scan_stderr);
        match run_cancellable(scan_cmd) {
            Ok(true) => {
                info!("Scan:   Done. Written to: {}", reference_list.display());
            }
            Ok(false) => {
                log::error!("msisensor-pro scan failed. See logs/scan.log");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                log::error!("{e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let baseline_dir = config.output_dir.join("baseline");

    // ── Parse CSV ──
    let mut reader = ReaderBuilder::new()
        .has_headers(false)
        .from_path(&pairs_file)
        .unwrap_or_else(|e| {
            log::error!("Failed to open {}: {e}", pairs_file.display());
            std::process::exit(1);
        });

    let all_samples: Vec<SamplePair> = reader
        .records()
        .filter_map(|r| {
            let record = r.ok()?;
            let tumor = record.get(0)?.trim().to_string();
            if tumor.is_empty() {
                return None;
            }
            let normal = record
                .get(1)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Some(SamplePair {
                tumor_bam: PathBuf::from(tumor),
                normal_bam: normal.map(PathBuf::from),
            })
        })
        .collect();

    let paired: Vec<&SamplePair> = all_samples
        .iter()
        .filter(|s| s.normal_bam.is_some())
        .collect();
    let tumor_only: Vec<&SamplePair> = all_samples
        .iter()
        .filter(|s| s.normal_bam.is_none())
        .collect();

    // ── Resume detection ──
    let already_done: usize = all_samples
        .iter()
        .filter(|s| checkpoint::sample_is_done(&config.output_dir, &s.tumor_bam))
        .count();
    let to_process: Vec<&SamplePair> = all_samples
        .iter()
        .filter(|s| !checkpoint::sample_is_done(&config.output_dir, &s.tumor_bam))
        .collect();

    info!(
        "Loaded {} samples: {} paired, {} tumor-only",
        all_samples.len(),
        paired.len(),
        tumor_only.len()
    );

    if already_done > 0 {
        info!(
            "Resuming: {already_done}/{} samples already completed, {} remaining",
            all_samples.len(),
            to_process.len()
        );
    }

    let needs_baseline = !tumor_only.is_empty();

    if needs_baseline && paired.is_empty() {
        log::error!("Tumor-only samples found but no paired samples to build baseline from.");
        log::error!("At least one paired (tumor,normal) sample is needed for baseline.");
        return ExitCode::FAILURE;
    }

    if needs_baseline {
        info!(
            "Tumor-only samples detected. Will build baseline from {} normal BAMs.",
            paired.len()
        );
    }
    std::thread::sleep(Duration::from_secs(2));

    // ── Enter TUI ──
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide);
    let _ = terminal::enable_raw_mode();

    let mut all_errors: Vec<String> = Vec::new();
    let mut baseline_file: Option<PathBuf> = None;
    let parallel_jobs = config.parallel_jobs;
    let threads_per_job = config.threads_per_job;

    // ── Collect unique normals ──
    let unique_normals: Vec<PathBuf> = if needs_baseline {
        let mut seen = HashSet::new();
        paired
            .iter()
            .filter_map(|s| s.normal_bam.as_ref())
            .filter(|n| seen.insert(n.to_string_lossy().to_string()))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    // ════════════════════════════════════════════════════════════════
    // PHASE 1: Profile normal samples (only if tumor-only exist)
    // ════════════════════════════════════════════════════════════════
    if needs_baseline {
        fs::create_dir_all(&baseline_dir).expect("Failed to create baseline directory");

        let normals_done: usize = unique_normals
            .iter()
            .filter(|n| checkpoint::normal_is_profiled(&config.output_dir, n))
            .count();
        let normals_todo: Vec<&PathBuf> = unique_normals
            .iter()
            .filter(|n| !checkpoint::normal_is_profiled(&config.output_dir, n))
            .collect();

        let phase1_label = format!(
            "PHASE 1/3: Profiling normal BAMs ({} todo, {} done)",
            normals_todo.len(),
            normals_done
        );
        let state = Arc::new(ProgressState::new(
            unique_normals.len(),
            parallel_jobs,
            &phase1_label,
        ));
        state.skipped.store(normals_done, Ordering::Relaxed);
        if normals_done > 0 {
            state.add_event(format!(
                "  RESUME  {normals_done} normal(s) already profiled"
            ));
        }

        let mut display = DisplayThread::start(Arc::clone(&state), parallel_jobs, normals_done);

        let output_dir = &config.output_dir;
        let phase1_errors =
            run_work_queue(&normals_todo, parallel_jobs, &state, |normal_bam, slot| {
                profile_normal(
                    normal_bam,
                    &baseline_dir,
                    &log_dir,
                    output_dir,
                    threads_per_job,
                    &reference_list,
                    &state,
                    slot,
                )?;
                Ok(())
            });

        display.stop();
        all_errors.extend(phase1_errors);

        if crate::is_cancelled() {
            let _ = terminal::disable_raw_mode();
            let _ = execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen);
            eprintln!("\nCancelled by user (Ctrl+C) during Phase 1.");
            return ExitCode::FAILURE;
        }

        // ════════════════════════════════════════════════════════════
        // PHASE 2: Build baseline
        // ════════════════════════════════════════════════════════════
        let baseline_output = baseline_dir.join("baseline_model");
        if baseline_output.exists() {
            let state2 = Arc::new(ProgressState::new(
                1,
                parallel_jobs,
                "PHASE 2/3: Baseline already built (resumed)",
            ));
            state2.skipped.store(1, Ordering::Relaxed);
            state2.add_event("  RESUME  Baseline model found from previous run".to_string());
            let mut display2 = DisplayThread::start(Arc::clone(&state2), parallel_jobs, 1);
            std::thread::sleep(Duration::from_secs(2));
            display2.stop();
            baseline_file = Some(baseline_output);
        } else {
            let phase2_label =
                "PHASE 2/3: Building baseline from normal profiles".to_string();
            let state2 = Arc::new(ProgressState::new(1, parallel_jobs, &phase2_label));
            state2.add_event("  INFO  Creating baseline configure file...".to_string());

            let mut display2 = DisplayThread::start(Arc::clone(&state2), parallel_jobs, 0);

            let config_path = baseline_dir.join("baseline_config.txt");
            {
                let mut config_file =
                    File::create(&config_path).expect("Failed to create baseline config");
                for normal_bam in &unique_normals {
                    let name = sample_name(normal_bam);
                    let all_path = baseline_dir.join(format!("{name}_all"));
                    if all_path.exists() {
                        writeln!(config_file, "{}\t{}", name, all_path.display())
                            .expect("Failed to write baseline config");
                    }
                }
            }

            state2.add_event("  INFO  Running msisensor-pro baseline...".to_string());
            state2.set_active(0, "baseline");

            let (bl_stdout, bl_stderr) = make_log_stdio(&log_dir, "baseline");

            let mut bl_cmd = Command::new("msisensor-pro");
            bl_cmd
                .args([
                    "baseline",
                    "-d",
                    &reference_list.to_string_lossy(),
                    "-i",
                    &config_path.to_string_lossy(),
                    "-o",
                    &baseline_output.to_string_lossy(),
                ])
                .stdout(bl_stdout)
                .stderr(bl_stderr);

            let bl_result = run_cancellable(bl_cmd);
            state2.clear_slot(0);

            match bl_result {
                Ok(true) => {
                    state2.completed.fetch_add(1, Ordering::Relaxed);
                    state2.add_event("  DONE  Baseline built successfully".to_string());
                    baseline_file = Some(baseline_output);
                }
                Ok(false) => {
                    state2.failed.fetch_add(1, Ordering::Relaxed);
                    state2.add_event(
                        "  FAIL  Baseline build exited with error".to_string(),
                    );
                    all_errors.push("Baseline build failed".to_string());
                }
                Err(e) => {
                    if e == "Cancelled" {
                        state2.add_event("  STOP  Baseline build cancelled".to_string());
                    } else {
                        state2.failed.fetch_add(1, Ordering::Relaxed);
                        state2.add_event(format!("  FAIL  {e}"));
                        all_errors.push(e);
                    }
                }
            }

            std::thread::sleep(Duration::from_secs(2));
            display2.stop();
        }

        if crate::is_cancelled() {
            let _ = terminal::disable_raw_mode();
            let _ = execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen);
            eprintln!("\nCancelled by user (Ctrl+C) during Phase 2.");
            return ExitCode::FAILURE;
        }
    }

    // ════════════════════════════════════════════════════════════════
    // PHASE 3 (or PHASE 1 if no tumor-only): MSI analysis
    // ════════════════════════════════════════════════════════════════
    let phase_num = if needs_baseline { "3/3" } else { "1/1" };
    let phase3_label = format!(
        "PHASE {}: MSI analysis ({} todo, {} done, {} paired, {} tumor-only)",
        phase_num,
        to_process.len(),
        already_done,
        paired.len(),
        tumor_only.len()
    );
    let state3 = Arc::new(ProgressState::new(
        all_samples.len(),
        parallel_jobs,
        &phase3_label,
    ));
    state3.skipped.store(already_done, Ordering::Relaxed);
    if already_done > 0 {
        state3.add_event(format!(
            "  RESUME  {already_done} sample(s) already completed"
        ));
    }

    if needs_baseline {
        if let Some(ref bf) = baseline_file {
            state3.add_event(format!("  INFO  Using baseline: {}", bf.display()));
        } else {
            state3.add_event(
                "  INFO  Baseline not available, tumor-only samples will run without baseline"
                    .to_string(),
            );
        }
    }

    let mut display3 = DisplayThread::start(Arc::clone(&state3), parallel_jobs, already_done);

    let output_dir = &config.output_dir;
    let phase3_errors = run_work_queue(&to_process, parallel_jobs, &state3, |pair, slot| {
        process_sample(
            output_dir,
            &log_dir,
            threads_per_job,
            pair,
            baseline_file.as_deref(),
            &reference_list,
            &state3,
            slot,
        )
    });

    all_errors.extend(phase3_errors);
    display3.stop();

    // ── Leave TUI ──
    let _ = terminal::disable_raw_mode();
    let _ = execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen);

    let was_cancelled = crate::is_cancelled();

    // ── Final summary ──
    let total = all_samples.len();
    let phase3_elapsed = fmt_duration(state3.start_time.elapsed());
    let failed = all_errors.len();
    let succeeded = total.saturating_sub(failed);

    println!();
    println!("  \u{2554}{}\u{2557}", "\u{2550}".repeat(46));
    if was_cancelled {
        println!(
            "  \u{2551}       MSI-CALC v{}  -  Cancelled by user    \u{2551}",
            VERSION
        );
    } else {
        println!(
            "  \u{2551}          MSI-CALC v{}  -  Run Complete       \u{2551}",
            VERSION
        );
    }
    println!("  \u{2560}{}\u{2563}", "\u{2550}".repeat(46));
    println!("  \u{2551}  Total samples:    {:<25}\u{2551}", total);
    println!("  \u{2551}  Paired:           {:<25}\u{2551}", paired.len());
    println!(
        "  \u{2551}  Tumor-only:       {:<25}\u{2551}",
        tumor_only.len()
    );
    if already_done > 0 {
        println!(
            "  \u{2551}  Resumed (skip):   {:<25}\u{2551}",
            already_done
        );
    }
    println!("  \u{2551}  Succeeded:        {:<25}\u{2551}", succeeded);
    println!("  \u{2551}  Failed:           {:<25}\u{2551}", failed);
    println!(
        "  \u{2551}  Elapsed:          {:<25}\u{2551}",
        phase3_elapsed
    );
    println!(
        "  \u{2551}  Jobs x Threads:   {:<25}\u{2551}",
        if config.resources_auto {
            format!(
                "{} x {} (auto)",
                config.parallel_jobs, config.threads_per_job
            )
        } else {
            format!("{} x {}", config.parallel_jobs, config.threads_per_job)
        }
    );
    if needs_baseline {
        let bl_status = if baseline_file.is_some() {
            "Built"
        } else {
            "Failed"
        };
        println!("  \u{2551}  Baseline:         {:<25}\u{2551}", bl_status);
    }
    println!("  \u{2560}{}\u{2563}", "\u{2550}".repeat(46));
    println!(
        "  \u{2551}  Results:  {:<35}\u{2551}",
        config.output_dir.display()
    );
    println!(
        "  \u{2551}  Logs:     {:<35}\u{2551}",
        log_dir.display()
    );
    if needs_baseline {
        println!(
            "  \u{2551}  Baseline: {:<35}\u{2551}",
            baseline_dir.display()
        );
    }
    println!("  \u{255A}{}\u{255D}", "\u{2550}".repeat(46));
    println!();

    if !all_errors.is_empty() {
        println!("  Errors:");
        for err in &all_errors {
            println!("    - {err}");
        }
        println!();
    }

    if was_cancelled {
        println!("  Run was cancelled. Re-run the same command to resume.");
        println!();
    } else if !all_errors.is_empty() {
        println!("  Some samples failed. Re-run to retry failed samples.");
        println!();
    }

    if all_errors.is_empty() && !was_cancelled {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
