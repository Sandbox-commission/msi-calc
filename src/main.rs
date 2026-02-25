use chrono::Local;
use crossterm::{
    cursor, event, execute,
    style::{self, Attribute, Color, Stylize},
    terminal::{self, ClearType},
};
use csv::ReaderBuilder;
use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static CANCELLED: AtomicBool = AtomicBool::new(false);

const DEFAULT_PAIRS: &str = "sample_pairs.csv";
const DEFAULT_OUTPUT: &str = "msi-results";
const NUM_THREADS: u32 = 2;
const PARALLEL_JOBS: usize = 16;
const REFRESH_INTERVAL: Duration = Duration::from_millis(100);

// ─── Help ───────────────────────────────────────────────────────────────────

fn usage() {
    eprintln!("msi-calc v0.3.0");
    eprintln!("Microsatellite instability (MSI) analysis using msisensor-pro");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    msi-calc -r <FASTA> [OPTIONS]");
    eprintln!();
    eprintln!("DESCRIPTION:");
    eprintln!("    Runs msisensor-pro on tumor/normal BAM pairs in parallel with a");
    eprintln!("    full-screen progress display and per-sample log files.");
    eprintln!();
    eprintln!("    Resume-aware: automatically detects previously completed samples");
    eprintln!("    and skips them, so you can safely re-run after interruption.");
    eprintln!();
    eprintln!("    Supports three modes depending on the input CSV:");
    eprintln!();
    eprintln!("    1. All paired (tumor + normal):");
    eprintln!("       Runs 'msisensor-pro msi' on each pair. Single phase.");
    eprintln!();
    eprintln!("    2. All tumor-only (no matched normal):");
    eprintln!("       Not supported. At least one paired sample is required");
    eprintln!("       to build the baseline for tumor-only detection.");
    eprintln!();
    eprintln!("    3. Mixed (paired + tumor-only in the same CSV):");
    eprintln!("       Automatically runs a 3-phase pipeline:");
    eprintln!("         Phase 1/3 - Profile normal BAMs from paired samples");
    eprintln!("                     using 'msisensor-pro pro' to generate");
    eprintln!("                     microsatellite distribution profiles.");
    eprintln!("         Phase 2/3 - Build a baseline model from the normal");
    eprintln!("                     profiles using 'msisensor-pro baseline'.");
    eprintln!("         Phase 3/3 - Run MSI analysis on all samples:");
    eprintln!("                     Paired     -> msisensor-pro msi (tumor vs normal)");
    eprintln!("                     Tumor-only -> msisensor-pro pro (using baseline)");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("    -r, --reference <FASTA> Path to reference genome FASTA (.fa/.fasta).");
    eprintln!("                            REQUIRED. Auto-runs 'msisensor-pro scan' on first");
    eprintln!("                            use to generate <output>/microsatellites.list.");
    eprintln!("                            The list is cached; subsequent runs skip scan.");
    eprintln!("    -p, --pairs <FILE>      Input CSV with tumor,normal BAM filenames");
    eprintln!("                            [default: sample_pairs.csv]");
    eprintln!("    -o, --output <DIR>      Directory to write MSI results");
    eprintln!("                            [default: msi-results]");
    eprintln!("    -j, --jobs <N>          Number of samples to process in parallel");
    eprintln!("                            [default: 16]");
    eprintln!("    -t, --threads <N>       Threads allocated per msisensor-pro job");
    eprintln!("                            [default: 2]");
    eprintln!("    -h, --help              Print this help message");
    eprintln!();
    eprintln!("INPUT FORMAT:");
    eprintln!("    CSV with no header. Each row: tumor_bam,normal_bam");
    eprintln!("    Leave the second column empty for tumor-only samples.");
    eprintln!("    Both types can be mixed in the same file.");
    eprintln!();
    eprintln!("    Example (sample_pairs.csv):");
    eprintln!("        50T_CRC_final.bam,50N_CRC_final.bam    # paired");
    eprintln!("        91T_CRC_final.bam,91N_CRC_final.bam    # paired");
    eprintln!("        78T_CRC_final.bam,                      # tumor-only");
    eprintln!("        19T_CRC_final.bam,                      # tumor-only");
    eprintln!();
    eprintln!("    Sample names are derived by stripping '_final.bam' from the");
    eprintln!("    tumor BAM filename (e.g. 50T_CRC_final.bam -> 50T_CRC).");
    eprintln!("    Already-processed samples are automatically skipped on resume.");
    eprintln!();
    eprintln!("REFERENCE:");
    eprintln!("    On first run, msi-calc scans the FASTA to generate a microsatellite loci");
    eprintln!("    list using: msisensor-pro scan -d <ref.fa> -o <output>/microsatellites.list");
    eprintln!("    The file is cached; subsequent runs skip the scan step automatically.");
    eprintln!("    Minimum coverage (-c):   20");
    eprintln!();
    eprintln!("OUTPUT STRUCTURE:");
    eprintln!("    <output>/");
    eprintln!("      <sample>              MSI result files per sample");
    eprintln!("      <sample>_msisensor    Additional msisensor output");
    eprintln!("      logs/                 Per-sample log files (.log)");
    eprintln!("      baseline/             (only when tumor-only samples present)");
    eprintln!("        baseline_config.txt   Configure file for baseline build");
    eprintln!("        baseline_model        Built baseline model");
    eprintln!("        <normal>_all          Normal sample profiles");
    eprintln!();
    eprintln!("EXAMPLES:");
    eprintln!("    # Basic run (reference is required)");
    eprintln!("    msi-calc -r /data/GRCh38.fa -p sample_pairs.csv");
    eprintln!();
    eprintln!("    # Specify output directory");
    eprintln!("    msi-calc -r /data/GRCh38.fa -p pairs.csv -o /data/msi-results");
    eprintln!();
    eprintln!("    # Use 8 parallel jobs with 4 threads each");
    eprintln!("    msi-calc -r /data/GRCh38.fa -p pairs.csv -j 8 -t 4");
    eprintln!();
    eprintln!("    # Resume an interrupted run (same command, scan step is skipped automatically)");
    eprintln!("    msi-calc -r /data/GRCh38.fa -p pairs.csv -o msi-results");
    eprintln!();
    eprintln!("NOTE:");
    eprintln!("    Total CPU usage = jobs x threads (default: 16 x 2 = 32 cores).");
    eprintln!("    Adjust -j and -t based on available resources.");
    eprintln!("    Press Ctrl+C to gracefully stop (kills running processes).");
}

// ─── Config & Args ──────────────────────────────────────────────────────────

struct Config {
    pairs_file: PathBuf,
    output_dir: PathBuf,
    parallel_jobs: usize,
    threads_per_job: u32,
    reference_fasta: PathBuf,
}

fn parse_args() -> Option<Config> {
    let args: Vec<String> = env::args().collect();
    let mut pairs_file = PathBuf::from(DEFAULT_PAIRS);
    let mut output_dir = PathBuf::from(DEFAULT_OUTPUT);
    let mut parallel_jobs = PARALLEL_JOBS;
    let mut threads_per_job = NUM_THREADS;
    let mut reference_fasta: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                usage();
                return None;
            }
            "-r" | "--reference" => {
                i += 1;
                reference_fasta = Some(PathBuf::from(&args[i]));
            }
            "-p" | "--pairs" => {
                i += 1;
                pairs_file = PathBuf::from(&args[i]);
            }
            "-o" | "--output" => {
                i += 1;
                output_dir = PathBuf::from(&args[i]);
            }
            "-j" | "--jobs" => {
                i += 1;
                parallel_jobs = args[i].parse().expect("Invalid value for --jobs");
            }
            "-t" | "--threads" => {
                i += 1;
                threads_per_job = args[i].parse().expect("Invalid value for --threads");
            }
            other => {
                eprintln!("Unknown option: {other}");
                usage();
                return None;
            }
        }
        i += 1;
    }

    let reference_fasta = match reference_fasta {
        Some(p) => p,
        None => {
            eprintln!("Error: -r / --reference <FASTA> is required.");
            eprintln!("       Provide the path to your reference genome (.fa or .fasta).");
            usage();
            return None;
        }
    };

    Some(Config {
        pairs_file,
        output_dir,
        parallel_jobs,
        threads_per_job,
        reference_fasta,
    })
}

fn is_cancelled() -> bool {
    CANCELLED.load(Ordering::Relaxed)
}

// ─── Data types ─────────────────────────────────────────────────────────────

struct SamplePair {
    tumor_bam: PathBuf,
    normal_bam: Option<PathBuf>,
}

fn sample_name(bam: &Path) -> String {
    bam.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .trim_end_matches("_final.bam")
        .to_string()
}

/// Path to the checkpoints directory inside output_dir
fn checkpoint_dir(output_dir: &Path) -> PathBuf {
    output_dir.join(".checkpoints")
}

/// Write a completion checkpoint for a sample
fn write_checkpoint(output_dir: &Path, name: &str) {
    let dir = checkpoint_dir(output_dir);
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.done"));
    let _ = File::create(&path);
}

/// Remove a checkpoint (on cancel/failure)
fn remove_checkpoint(output_dir: &Path, name: &str) {
    let path = checkpoint_dir(output_dir).join(format!("{name}.done"));
    let _ = fs::remove_file(&path);
}

/// Check if a sample completed successfully (checkpoint exists)
fn sample_is_done(output_dir: &Path, pair: &SamplePair) -> bool {
    let name = sample_name(&pair.tumor_bam);
    checkpoint_dir(output_dir).join(format!("{name}.done")).exists()
}

/// Check if a normal BAM has been profiled (checkpoint exists)
fn normal_is_profiled(output_dir: &Path, normal_bam: &Path) -> bool {
    let name = sample_name(normal_bam);
    checkpoint_dir(output_dir)
        .join(format!("{name}_profile.done"))
        .exists()
}

/// Write a profiling checkpoint for a normal sample
fn write_profile_checkpoint(output_dir: &Path, name: &str) {
    let dir = checkpoint_dir(output_dir);
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}_profile.done"));
    let _ = File::create(&path);
}

/// Remove a profiling checkpoint
fn remove_profile_checkpoint(output_dir: &Path, name: &str) {
    let path = checkpoint_dir(output_dir).join(format!("{name}_profile.done"));
    let _ = fs::remove_file(&path);
}

// ─── Shared progress state ─────────────────────────────────────────────────

#[derive(Clone)]
struct JobSlot {
    sample: String,
    started: Instant,
}

struct ProgressState {
    total: usize,
    completed: AtomicUsize,
    skipped: AtomicUsize,
    failed: AtomicUsize,
    active_jobs: Mutex<Vec<Option<JobSlot>>>,
    recent_events: Mutex<Vec<String>>,
    phase_label: Mutex<String>,
    start_time: Instant,
    completed_durations: Mutex<Vec<f64>>,
}

impl ProgressState {
    fn new(total: usize, parallel_jobs: usize, phase: &str) -> Self {
        let slots = (0..parallel_jobs).map(|_| None).collect();
        Self {
            total,
            completed: AtomicUsize::new(0),
            skipped: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            active_jobs: Mutex::new(slots),
            recent_events: Mutex::new(Vec::new()),
            phase_label: Mutex::new(phase.to_string()),
            start_time: Instant::now(),
            completed_durations: Mutex::new(Vec::new()),
        }
    }

    fn set_active(&self, slot: usize, sample: &str) {
        if let Ok(mut jobs) = self.active_jobs.lock() {
            if slot < jobs.len() {
                jobs[slot] = Some(JobSlot {
                    sample: sample.to_string(),
                    started: Instant::now(),
                });
            }
        }
    }

    fn clear_slot(&self, slot: usize) {
        if let Ok(mut jobs) = self.active_jobs.lock() {
            if slot < jobs.len() {
                jobs[slot] = None;
            }
        }
    }

    fn add_event(&self, msg: String) {
        if let Ok(mut events) = self.recent_events.lock() {
            events.push(msg);
            if events.len() > 100 {
                events.remove(0);
            }
        }
    }

    fn done_count(&self) -> usize {
        self.completed.load(Ordering::Relaxed)
            + self.skipped.load(Ordering::Relaxed)
            + self.failed.load(Ordering::Relaxed)
    }

    fn record_duration(&self, secs: f64) {
        if let Ok(mut durations) = self.completed_durations.lock() {
            durations.push(secs);
        }
    }

    fn avg_duration(&self) -> f64 {
        self.completed_durations
            .lock()
            .ok()
            .and_then(|d| {
                if d.is_empty() {
                    None
                } else {
                    Some(d.iter().sum::<f64>() / d.len() as f64)
                }
            })
            .unwrap_or(0.0)
    }

    fn phase(&self) -> String {
        self.phase_label.lock().map(|p| p.clone()).unwrap_or_default()
    }
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

/// Remove partial output files left by a killed/failed msisensor-pro run.
/// msisensor-pro creates: <name>, <name>_dis, <name>_germline, <name>_somatic, <name>_all
fn cleanup_partial_output(dir: &Path, name: &str) {
    let suffixes = ["", "_dis", "_germline", "_somatic", "_all", "_unstable"];
    for suffix in &suffixes {
        let path = dir.join(format!("{name}{suffix}"));
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
    }
    // Also remove the _msisensor variant
    let msi_path = dir.join(format!("{name}_msisensor"));
    if msi_path.exists() {
        let _ = fs::remove_file(&msi_path);
    }
}

// ─── Run command with cancellation support ──────────────────────────────────

/// Spawn a command and poll it, killing the child if Ctrl+C is pressed.
/// Returns Ok(true) on success, Ok(false) on non-zero exit, Err on spawn failure.
fn run_cancellable(mut cmd: Command) -> Result<bool, String> {
    let mut child = cmd.spawn().map_err(|e| format!("Failed to launch: {e}"))?;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) => {
                if is_cancelled() {
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
    if is_cancelled() {
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
        "-d", &reference_list.to_string_lossy(),
        "-c", "20",
        "-t", &normal_bam.to_string_lossy(),
        "-b", &threads_str,
        "-o", &output_str,
    ])
    .stdout(stdout_cfg)
    .stderr(stderr_cfg);

    let result = run_cancellable(cmd);
    let dur = job_start.elapsed().as_secs_f64();
    // Note: clear_slot is handled by the work queue after this returns

    match result {
        Ok(true) => {
            write_profile_checkpoint(output_dir, &name);
            state.record_duration(dur);
            state.completed.fetch_add(1, Ordering::Relaxed);
            state.add_event(format!("  DONE  {name} ({:.0}s)", dur));
            Ok(all_file)
        }
        Ok(false) => {
            cleanup_partial_output(baseline_dir, &name);
            remove_profile_checkpoint(output_dir, &name);
            state.failed.fetch_add(1, Ordering::Relaxed);
            state.add_event(format!("  FAIL  {name} (partial output cleaned)"));
            Err(format!("[{name}] msisensor-pro pro exited with error"))
        }
        Err(e) => {
            cleanup_partial_output(baseline_dir, &name);
            remove_profile_checkpoint(output_dir, &name);
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
    if is_cancelled() {
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
            "-d", &reference_list.to_string_lossy(),
            "-c", "20",
            "-n", &normal.to_string_lossy(),
            "-t", &pair.tumor_bam.to_string_lossy(),
            "-b", &threads_str,
            "-o", &output_str,
        ]);
    } else {
        let d_file = match baseline_file {
            Some(bf) => bf.to_string_lossy().to_string(),
            None => reference_list.to_string_lossy().to_string(),
        };
        cmd.args([
            "pro",
            "-d", &d_file,
            "-c", "20",
            "-t", &pair.tumor_bam.to_string_lossy(),
            "-b", &threads_str,
            "-o", &output_str,
        ]);
    }

    cmd.stdout(stdout_cfg).stderr(stderr_cfg);

    let result = run_cancellable(cmd);
    let dur = job_start.elapsed().as_secs_f64();
    // Note: clear_slot is handled by the work queue after this returns

    match result {
        Ok(true) => {
            write_checkpoint(output_dir, &sample);
            state.record_duration(dur);
            state.completed.fetch_add(1, Ordering::Relaxed);
            state.add_event(format!("  DONE  {sample} ({:.0}s)", dur));
            Ok(())
        }
        Ok(false) => {
            cleanup_partial_output(output_dir, &sample);
            remove_checkpoint(output_dir, &sample);
            state.failed.fetch_add(1, Ordering::Relaxed);
            state.add_event(format!("  FAIL  {sample} (partial output cleaned)"));
            Err(format!("[{sample}] msisensor-pro exited with error"))
        }
        Err(e) => {
            cleanup_partial_output(output_dir, &sample);
            remove_checkpoint(output_dir, &sample);
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

// ─── TUI rendering ─────────────────────────────────────────────────────────

fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

fn fmt_secs(s: f64) -> String {
    fmt_duration(Duration::from_secs_f64(s))
}

fn render_screen(
    stdout: &mut io::Stdout,
    state: &ProgressState,
    parallel_jobs: usize,
    resumed: usize,
) {
    let (term_w, term_h) = terminal::size().unwrap_or((80, 24));
    let w = term_w as usize;
    let h = term_h as usize;

    let elapsed = state.start_time.elapsed();
    let done = state.done_count();
    let total = state.total;
    let completed = state.completed.load(Ordering::Relaxed);
    let skipped = state.skipped.load(Ordering::Relaxed);
    let failed = state.failed.load(Ordering::Relaxed);
    let remaining = total.saturating_sub(done);
    let phase = state.phase();
    let avg_dur = state.avg_duration();

    let pct = if total > 0 { done * 100 / total } else { 0 };
    // Speed based only on actually-processed (not resumed/skipped)
    let processed = completed;
    let speed = if elapsed.as_secs() > 0 && processed > 0 {
        processed as f64 / (elapsed.as_secs_f64() / 60.0)
    } else {
        0.0
    };
    let eta = if processed > 0 && remaining > 0 {
        Duration::from_secs_f64(avg_dur * remaining as f64)
    } else if done > 0 && remaining > 0 {
        let per = elapsed.as_secs_f64() / done as f64;
        Duration::from_secs_f64(per * remaining as f64)
    } else {
        Duration::ZERO
    };

    let bar_width = w.saturating_sub(2).min(60);
    let filled = if total > 0 { bar_width * done / total } else { 0 };
    let empty = bar_width.saturating_sub(filled);
    let bar_filled: String = "\u{2588}".repeat(filled);
    let bar_empty: String = "\u{2591}".repeat(empty);

    let active_snapshot: Vec<Option<JobSlot>> = state
        .active_jobs
        .lock()
        .map(|j| j.clone())
        .unwrap_or_default();
    let active_count = active_snapshot.iter().filter(|s| s.is_some()).count();

    let events: Vec<String> = state
        .recent_events
        .lock()
        .map(|e| e.clone())
        .unwrap_or_default();

    let _ = execute!(stdout, cursor::MoveTo(0, 0), terminal::Clear(ClearType::All));

    let border_top = format!("\u{2554}{}\u{2557}", "\u{2550}".repeat(w.saturating_sub(2)));
    let border_mid = format!("\u{2560}{}\u{2563}", "\u{2550}".repeat(w.saturating_sub(2)));
    let border_bot = format!("\u{255A}{}\u{255D}", "\u{2550}".repeat(w.saturating_sub(2)));

    let mut row: u16 = 0;

    macro_rules! bline {
        ($color:expr, $text:expr) => {{
            let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
            let txt: String = $text;
            let txt_len = txt.len().min(w.saturating_sub(2));
            print!("\u{2551}");
            let _ = execute!(stdout, style::SetForegroundColor($color));
            print!("{}", &txt[..txt_len]);
            let _ = execute!(stdout, style::SetForegroundColor(Color::Cyan));
            print!("{}\u{2551}", " ".repeat(w.saturating_sub(2).saturating_sub(txt_len)));
            row += 1;
        }};
    }

    macro_rules! separator {
        () => {{
            let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
            print!("{border_mid}");
            row += 1;
        }};
    }

    // Top border
    let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
    print!("{border_top}");
    row += 1;

    // Header
    let title = "MSI-CALC v0.3.0";
    let subtitle = "Microsatellite Instability Analysis | msisensor-pro";
    let pad_title = (w.saturating_sub(2).saturating_sub(title.len())) / 2;
    let pad_sub = (w.saturating_sub(2).saturating_sub(subtitle.len())) / 2;

    let _ = execute!(stdout, cursor::MoveTo(0, row));
    print!("\u{2551}{}{}{}\u{2551}",
        " ".repeat(pad_title),
        title.with(Color::White).attribute(Attribute::Bold),
        " ".repeat(w.saturating_sub(2).saturating_sub(pad_title).saturating_sub(title.len()))
    );
    row += 1;

    let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
    print!("\u{2551}{}{}{}\u{2551}",
        " ".repeat(pad_sub),
        subtitle.with(Color::DarkGrey),
        " ".repeat(w.saturating_sub(2).saturating_sub(pad_sub).saturating_sub(subtitle.len()))
    );
    row += 1;

    separator!();

    // Phase indicator
    let phase_line = format!("  {phase}");
    bline!(Color::Magenta, phase_line);

    // Show resume info if applicable
    if resumed > 0 {
        let resume_line = format!("  Resumed: {resumed} sample(s) already completed from previous run");
        bline!(Color::DarkYellow, resume_line);
    }

    separator!();

    // Overall progress bar
    bline!(Color::White, "  OVERALL PROGRESS".to_string());

    let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
    print!("\u{2551}  ");
    let _ = execute!(stdout, style::SetForegroundColor(Color::Green));
    print!("{bar_filled}");
    let _ = execute!(stdout, style::SetForegroundColor(Color::DarkGrey));
    print!("{bar_empty}");
    let _ = execute!(stdout, style::SetForegroundColor(Color::White));
    let pct_str = format!(" {:>3}%", pct);
    print!("{pct_str}");
    let used = 2 + bar_width + pct_str.len();
    let _ = execute!(stdout, style::SetForegroundColor(Color::Cyan));
    print!("{}\u{2551}", " ".repeat(w.saturating_sub(2).saturating_sub(used)));
    row += 1;

    let stats_line = format!(
        "  {}/{} done   Elapsed: {}   ETA: {}   Speed: {:.1}/min",
        done, total, fmt_duration(elapsed), fmt_duration(eta), speed
    );
    let stats_len = stats_line.len();
    let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
    print!("\u{2551}{}{}\u{2551}", stats_line.with(Color::White), " ".repeat(w.saturating_sub(2).saturating_sub(stats_len)));
    row += 1;

    separator!();

    // Active jobs with per-sample progress bars (only show running jobs)
    let active_label = format!("  ACTIVE JOBS ({}/{})", active_count, parallel_jobs);
    let active_label_len = active_label.len();
    let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
    print!("\u{2551}{}{}\u{2551}", active_label.with(Color::Yellow), " ".repeat(w.saturating_sub(2).saturating_sub(active_label_len)));
    row += 1;

    // Collect only active jobs (skip empty slots)
    let active_jobs: Vec<(usize, &JobSlot)> = active_snapshot
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.as_ref().map(|j| (i, j)))
        .collect();

    let max_active_rows = (h.saturating_sub(row as usize + 12)) / 2;
    let spinner_chars = ['|', '/', '-', '\\'];
    let spin_idx = (elapsed.as_millis() / 250) as usize;
    let sample_bar_w = w.saturating_sub(8).min(40);

    if active_jobs.is_empty() {
        bline!(Color::DarkGrey, "  No active jobs".to_string());
    }

    for (shown, (i, job)) in active_jobs.iter().enumerate() {
        if shown >= max_active_rows {
            let hidden = active_jobs.len().saturating_sub(shown);
            if hidden > 0 {
                let more = format!("  ... and {hidden} more active");
                let more_len = more.len();
                let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
                print!("\u{2551}{}{}\u{2551}", more.with(Color::DarkGrey), " ".repeat(w.saturating_sub(2).saturating_sub(more_len)));
                row += 1;
            }
            break;
        }

        let spin = spinner_chars[(spin_idx + i) % 4];
        let job_elapsed_secs = job.started.elapsed().as_secs_f64();
        let job_elapsed_str = fmt_secs(job_elapsed_secs);
        let max_name = w.saturating_sub(24).min(35);
        let name = if job.sample.len() > max_name {
            format!("{}...", &job.sample[..max_name.saturating_sub(3)])
        } else {
            job.sample.clone()
        };

        // Row 1: spinner + name + elapsed / ETA
        let eta_part = if avg_dur > 0.0 {
            format!("{} / ~{}", job_elapsed_str, fmt_secs(avg_dur))
        } else {
            job_elapsed_str
        };
        let line = format!("  {spin} {name:<max_name$} {eta_part}");
        let display = if line.len() > w.saturating_sub(2) {
            line[..w.saturating_sub(2)].to_string()
        } else {
            line
        };
        let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
        print!("\u{2551}");
        let _ = execute!(stdout, style::SetForegroundColor(Color::White));
        print!("{display}");
        let _ = execute!(stdout, style::SetForegroundColor(Color::Cyan));
        print!("{}\u{2551}", " ".repeat(w.saturating_sub(2).saturating_sub(display.len())));
        row += 1;

        // Row 2: per-sample progress bar
        let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
        print!("\u{2551}    ");
        if avg_dur > 0.0 {
            let frac = (job_elapsed_secs / avg_dur).min(1.0);
            let s_filled = (sample_bar_w as f64 * frac) as usize;
            let s_empty = sample_bar_w.saturating_sub(s_filled);
            let _ = execute!(stdout, style::SetForegroundColor(Color::Yellow));
            print!("{}", "\u{2588}".repeat(s_filled));
            let _ = execute!(stdout, style::SetForegroundColor(Color::DarkGrey));
            print!("{}", "\u{2591}".repeat(s_empty));
            let _ = execute!(stdout, style::SetForegroundColor(Color::White));
            let s_pct = format!(" {:>3}%", (frac * 100.0) as usize);
            print!("{s_pct}");
            let bar_used = 4 + sample_bar_w + s_pct.len();
            let _ = execute!(stdout, style::SetForegroundColor(Color::Cyan));
            print!("{}\u{2551}", " ".repeat(w.saturating_sub(2).saturating_sub(bar_used)));
        } else {
            // Indeterminate: pulse animation
            let pulse_pos = (spin_idx + i * 3) % (sample_bar_w + 4);
            for p in 0..sample_bar_w {
                if p >= pulse_pos.saturating_sub(2) && p <= pulse_pos {
                    let _ = execute!(stdout, style::SetForegroundColor(Color::Yellow));
                    print!("\u{2588}");
                } else {
                    let _ = execute!(stdout, style::SetForegroundColor(Color::DarkGrey));
                    print!("\u{2591}");
                }
            }
            let bar_used = 4 + sample_bar_w;
            let _ = execute!(stdout, style::SetForegroundColor(Color::Cyan));
            print!("{}\u{2551}", " ".repeat(w.saturating_sub(2).saturating_sub(bar_used)));
        }
        row += 1;
    }

    separator!();

    // Counters
    let counters = format!(
        "  Completed: {}   Skipped: {}   Failed: {}   Remaining: {}",
        completed, skipped, failed, remaining
    );
    let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
    print!("\u{2551}");
    let _ = execute!(stdout, style::SetForegroundColor(Color::Green));
    print!("  Completed: {completed}");
    let _ = execute!(stdout, style::SetForegroundColor(Color::Yellow));
    print!("   Skipped: {skipped}");
    let _ = execute!(stdout, style::SetForegroundColor(Color::Red));
    print!("   Failed: {failed}");
    let _ = execute!(stdout, style::SetForegroundColor(Color::White));
    print!("   Remaining: {remaining}");
    let used_len = counters.len();
    let _ = execute!(stdout, style::SetForegroundColor(Color::Cyan));
    print!("{}\u{2551}", " ".repeat(w.saturating_sub(2).saturating_sub(used_len)));
    row += 1;

    separator!();

    // Recent activity
    let log_label = "  RECENT ACTIVITY";
    let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
    print!("\u{2551}{}{}\u{2551}", log_label.with(Color::Magenta), " ".repeat(w.saturating_sub(2).saturating_sub(log_label.len())));
    row += 1;

    let max_event_rows = h.saturating_sub(row as usize + 3);
    let start = events.len().saturating_sub(max_event_rows);
    for event in &events[start..] {
        let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
        let ev = if event.len() > w.saturating_sub(2) {
            event[..w.saturating_sub(2)].to_string()
        } else {
            event.clone()
        };
        print!("\u{2551}");
        if ev.contains("DONE") {
            let _ = execute!(stdout, style::SetForegroundColor(Color::Green));
        } else if ev.contains("SKIP") || ev.contains("RESUME") {
            let _ = execute!(stdout, style::SetForegroundColor(Color::Yellow));
        } else if ev.contains("FAIL") {
            let _ = execute!(stdout, style::SetForegroundColor(Color::Red));
        } else if ev.contains("STOP") {
            let _ = execute!(stdout, style::SetForegroundColor(Color::DarkRed));
        } else if ev.contains("INFO") {
            let _ = execute!(stdout, style::SetForegroundColor(Color::Cyan));
        } else {
            let _ = execute!(stdout, style::SetForegroundColor(Color::White));
        }
        print!("{ev}");
        let _ = execute!(stdout, style::SetForegroundColor(Color::Cyan));
        print!("{}\u{2551}", " ".repeat(w.saturating_sub(2).saturating_sub(ev.len())));
        row += 1;
    }

    // Fill remaining
    while (row as usize) < h.saturating_sub(2) {
        let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
        print!("\u{2551}{}\u{2551}", " ".repeat(w.saturating_sub(2)));
        row += 1;
    }

    // Footer
    let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
    let cancel_hint = if is_cancelled() { "  CANCELLING..." } else { "  Ctrl+C to cancel" };
    let timestamp = format!("Updated: {} ", Local::now().format("%H:%M:%S"));
    let footer_pad = w.saturating_sub(2).saturating_sub(cancel_hint.len()).saturating_sub(timestamp.len());
    if is_cancelled() {
        print!("\u{2551}{}{}{}\u{2551}",
            cancel_hint.with(Color::Red),
            " ".repeat(footer_pad),
            timestamp.with(Color::DarkGrey)
        );
    } else {
        print!("\u{2551}{}{}{}\u{2551}",
            cancel_hint.with(Color::DarkGrey),
            " ".repeat(footer_pad),
            timestamp.with(Color::DarkGrey)
        );
    }
    row += 1;

    let _ = execute!(stdout, cursor::MoveTo(0, row), style::SetForegroundColor(Color::Cyan));
    print!("{border_bot}");

    let _ = execute!(stdout, style::ResetColor);
    let _ = stdout.flush();
}

// ─── Display thread control ─────────────────────────────────────────────────

struct DisplayThread {
    flag: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl DisplayThread {
    fn start(state: Arc<ProgressState>, parallel_jobs: usize, resumed: usize) -> Self {
        let flag = Arc::new(AtomicBool::new(false));
        let display_flag = Arc::clone(&flag);

        let handle = std::thread::spawn(move || {
            let mut out = io::stdout();
            loop {
                render_screen(&mut out, &state, parallel_jobs, resumed);

                // Poll for Ctrl+C key press (raw mode blocks SIGINT)
                if event::poll(REFRESH_INTERVAL).unwrap_or(false) {
                    if let Ok(event::Event::Key(key)) = event::read() {
                        if key.code == event::KeyCode::Char('c')
                            && key.modifiers.contains(event::KeyModifiers::CONTROL)
                        {
                            CANCELLED.store(true, Ordering::Relaxed);
                        }
                    }
                }

                if display_flag.load(Ordering::Relaxed) || is_cancelled() {
                    render_screen(&mut out, &state, parallel_jobs, resumed);
                    break;
                }
            }
        });

        Self {
            flag,
            handle: Some(handle),
        }
    }

    fn stop(&mut self) {
        self.flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// ─── Work queue runner ──────────────────────────────────────────────────────

/// Run items through a work queue with N threads. Each thread grabs the next
/// item as soon as it finishes one - no chunking delays.
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
                    if is_cancelled() {
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
                    // Clear slot and briefly yield so display thread can see it
                    st.clear_slot(slot);
                    std::thread::yield_now();
                }
            });
        }
    });

    errors.into_inner().unwrap()
}

// ─── Interactive prompt ─────────────────────────────────────────────────────

fn prompt_for_pairs(config_pairs: PathBuf) -> Option<PathBuf> {
    let user_specified = env::args().any(|a| a == "-p" || a == "--pairs");
    if user_specified {
        if !config_pairs.exists() {
            eprintln!("File not found: {}", config_pairs.display());
            return None;
        }
        return Some(config_pairs);
    }

    let cwd = env::current_dir().unwrap_or_default();
    let candidate = cwd.join("sample_pairs.csv");
    let found = candidate.exists();

    if found {
        eprintln!("Found sample_pairs.csv in current directory: {}", cwd.display());
        eprintln!();
        eprintln!("  [y] Use this file and proceed");
        eprintln!("  [n] Enter a different path");
        eprintln!("  [q] Quit");
        eprint!("\nProceed? [y/n/q]: ");
    } else {
        eprintln!("  [1] Enter a path to the sample pairs CSV");
        eprintln!("  [q] Quit");
        eprint!("\nYour choice: ");
    }
    io::stderr().flush().ok();

    let mut choice = String::new();
    io::stdin().lock().read_line(&mut choice).ok();
    let choice = choice.trim();

    if found {
        match choice {
            "y" | "Y" => Some(candidate),
            "n" | "N" => prompt_path(),
            _ => None,
        }
    } else {
        match choice {
            "1" => prompt_path(),
            _ => None,
        }
    }
}

fn prompt_path() -> Option<PathBuf> {
    eprint!("Enter path to sample pairs CSV: ");
    io::stderr().flush().ok();
    let mut path = String::new();
    io::stdin().lock().read_line(&mut path).ok();
    let path = PathBuf::from(path.trim());
    if !path.exists() {
        eprintln!("File not found: {}", path.display());
        return None;
    }
    Some(path)
}

// ─── Main ───────────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    let config = match parse_args() {
        Some(c) => c,
        None => return ExitCode::SUCCESS,
    };

    eprintln!("msi-calc v0.3.0 | Run with -h or --help for usage information");
    eprintln!();

    let pairs_file = match prompt_for_pairs(config.pairs_file) {
        Some(p) => p,
        None => return ExitCode::FAILURE,
    };

    run(
        pairs_file,
        config.output_dir,
        config.parallel_jobs,
        config.threads_per_job,
        config.reference_fasta,
    )
}

fn run(
    pairs_file: PathBuf,
    output_dir: PathBuf,
    parallel_jobs: usize,
    threads_per_job: u32,
    reference_fasta: PathBuf,
) -> ExitCode {
    fs::create_dir_all(&output_dir).expect("Failed to create output directory");
    let log_dir = output_dir.join("logs");
    fs::create_dir_all(&log_dir).expect("Failed to create logs directory");
    fs::create_dir_all(checkpoint_dir(&output_dir)).expect("Failed to create checkpoints directory");

    // ── Phase 0: Scan reference to generate microsatellites.list ──────────────
    if !reference_fasta.exists() {
        eprintln!("Error: Reference FASTA not found: {}", reference_fasta.display());
        return ExitCode::FAILURE;
    }
    let reference_list = output_dir.join("microsatellites.list");
    if reference_list.exists() {
        eprintln!("Scan:   microsatellites.list already present, skipping scan.");
        eprintln!("        {}", reference_list.display());
    } else {
        eprintln!("Scan:   generating microsatellites.list from reference FASTA...");
        eprintln!("        msisensor-pro scan -d {} -o {}",
            reference_fasta.display(), reference_list.display());
        let (scan_stdout, scan_stderr) = make_log_stdio(&log_dir, "scan");
        let mut scan_cmd = Command::new("msisensor-pro");
        scan_cmd.args([
            "scan",
            "-d", &reference_fasta.to_string_lossy(),
            "-o", &reference_list.to_string_lossy(),
        ])
        .stdout(scan_stdout)
        .stderr(scan_stderr);
        match run_cancellable(scan_cmd) {
            Ok(true) => {
                eprintln!("Scan:   Done. Written to: {}", reference_list.display());
            }
            Ok(false) => {
                eprintln!("Error: msisensor-pro scan failed. See logs/scan.log");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("Error: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    eprintln!();

    let baseline_dir = output_dir.join("baseline");

    // ── Parse CSV ──
    let mut reader = ReaderBuilder::new()
        .has_headers(false)
        .from_path(&pairs_file)
        .unwrap_or_else(|e| {
            eprintln!("Failed to open {}: {e}", pairs_file.display());
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

    let paired: Vec<&SamplePair> = all_samples.iter().filter(|s| s.normal_bam.is_some()).collect();
    let tumor_only: Vec<&SamplePair> = all_samples.iter().filter(|s| s.normal_bam.is_none()).collect();

    // ── Resume detection: count already-completed samples ──
    let already_done: usize = all_samples.iter().filter(|s| sample_is_done(&output_dir, s)).count();
    let to_process: Vec<&SamplePair> = all_samples.iter().filter(|s| !sample_is_done(&output_dir, s)).collect();

    eprintln!("Loaded {} samples: {} paired, {} tumor-only",
        all_samples.len(), paired.len(), tumor_only.len());

    if already_done > 0 {
        eprintln!("Resuming: {already_done}/{} samples already completed, {} remaining",
            all_samples.len(), to_process.len());
    }

    let needs_baseline = !tumor_only.is_empty();

    if needs_baseline && paired.is_empty() {
        eprintln!("Error: Tumor-only samples found but no paired samples to build baseline from.");
        eprintln!("       At least one paired (tumor,normal) sample is needed for baseline.");
        return ExitCode::FAILURE;
    }

    if needs_baseline {
        eprintln!("Tumor-only samples detected. Will build baseline from {} normal BAMs.", paired.len());
    }
    eprintln!();
    std::thread::sleep(Duration::from_secs(2));

    // ── Enter TUI ──
    let mut stdout = io::stdout();
    let _ = execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide);
    let _ = terminal::enable_raw_mode();

    let mut all_errors: Vec<String> = Vec::new();
    let mut baseline_file: Option<PathBuf> = None;

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

        // Resume: separate already-profiled from needing-work
        let normals_done: usize = unique_normals.iter()
            .filter(|n| normal_is_profiled(&output_dir, n))
            .count();
        let normals_todo: Vec<&PathBuf> = unique_normals.iter()
            .filter(|n| !normal_is_profiled(&output_dir, n))
            .collect();

        let phase1_label = format!(
            "PHASE 1/3: Profiling normal BAMs ({} todo, {} done)",
            normals_todo.len(), normals_done
        );
        let state = Arc::new(ProgressState::new(
            unique_normals.len(), parallel_jobs, &phase1_label
        ));
        // Pre-set skipped count for resumed normals
        state.skipped.store(normals_done, Ordering::Relaxed);
        if normals_done > 0 {
            state.add_event(format!("  RESUME  {normals_done} normal(s) already profiled"));
        }

        let mut display = DisplayThread::start(Arc::clone(&state), parallel_jobs, normals_done);

        let phase1_errors = run_work_queue(&normals_todo, parallel_jobs, &state, |normal_bam, slot| {
            profile_normal(normal_bam, &baseline_dir, &log_dir, &output_dir, threads_per_job, &reference_list, &state, slot)?;
            Ok(())
        });

        display.stop();
        all_errors.extend(phase1_errors);

        if is_cancelled() {
            let _ = terminal::disable_raw_mode();
            let _ = execute!(stdout, cursor::Show, terminal::LeaveAlternateScreen);
            eprintln!("\nCancelled by user (Ctrl+C) during Phase 1.");
            return ExitCode::FAILURE;
        }

        // ════════════════════════════════════════════════════════════
        // PHASE 2: Build baseline
        // ════════════════════════════════════════════════════════════

        // Check if baseline already exists (resume)
        let baseline_output = baseline_dir.join("baseline_model");
        if baseline_output.exists() {
            let state2 = Arc::new(ProgressState::new(1, parallel_jobs,
                "PHASE 2/3: Baseline already built (resumed)"));
            state2.skipped.store(1, Ordering::Relaxed);
            state2.add_event("  RESUME  Baseline model found from previous run".to_string());
            let mut display2 = DisplayThread::start(Arc::clone(&state2), parallel_jobs, 1);
            std::thread::sleep(Duration::from_secs(2));
            display2.stop();
            baseline_file = Some(baseline_output);
        } else {
            let phase2_label = "PHASE 2/3: Building baseline from normal profiles".to_string();
            let state2 = Arc::new(ProgressState::new(1, parallel_jobs, &phase2_label));
            state2.add_event("  INFO  Creating baseline configure file...".to_string());

            let mut display2 = DisplayThread::start(Arc::clone(&state2), parallel_jobs, 0);

            let config_path = baseline_dir.join("baseline_config.txt");
            {
                let mut config_file = File::create(&config_path).expect("Failed to create baseline config");
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
            bl_cmd.args([
                "baseline",
                "-d", &reference_list.to_string_lossy(),
                "-i", &config_path.to_string_lossy(),
                "-o", &baseline_output.to_string_lossy(),
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
                    state2.add_event("  FAIL  Baseline build exited with error".to_string());
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

        if is_cancelled() {
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
    let state3 = Arc::new(ProgressState::new(all_samples.len(), parallel_jobs, &phase3_label));
    // Pre-set skipped count for already-done samples
    state3.skipped.store(already_done, Ordering::Relaxed);
    if already_done > 0 {
        state3.add_event(format!("  RESUME  {already_done} sample(s) already completed"));
    }

    if needs_baseline {
        if let Some(ref bf) = baseline_file {
            state3.add_event(format!("  INFO  Using baseline: {}", bf.display()));
        } else {
            state3.add_event("  INFO  Baseline not available, tumor-only samples will run without baseline".to_string());
        }
    }

    let mut display3 = DisplayThread::start(Arc::clone(&state3), parallel_jobs, already_done);

    let phase3_errors = run_work_queue(&to_process, parallel_jobs, &state3, |pair, slot| {
        process_sample(
            &output_dir,
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

    let was_cancelled = is_cancelled();

    // ── Final summary ──
    let total = all_samples.len();
    let phase3_elapsed = fmt_duration(state3.start_time.elapsed());
    let failed = all_errors.len();
    let succeeded = total.saturating_sub(failed);

    println!();
    println!("  \u{2554}{}\u{2557}", "\u{2550}".repeat(46));
    if was_cancelled {
        println!("  \u{2551}       MSI-CALC  -  Cancelled by user        \u{2551}");
    } else {
        println!("  \u{2551}          MSI-CALC  -  Run Complete           \u{2551}");
    }
    println!("  \u{2560}{}\u{2563}", "\u{2550}".repeat(46));
    println!("  \u{2551}  Total samples:    {:<25}\u{2551}", total);
    println!("  \u{2551}  Paired:           {:<25}\u{2551}", paired.len());
    println!("  \u{2551}  Tumor-only:       {:<25}\u{2551}", tumor_only.len());
    if already_done > 0 {
        println!("  \u{2551}  Resumed (skip):   {:<25}\u{2551}", already_done);
    }
    println!("  \u{2551}  Succeeded:        {:<25}\u{2551}", succeeded);
    println!("  \u{2551}  Failed:           {:<25}\u{2551}", failed);
    println!("  \u{2551}  Elapsed:          {:<25}\u{2551}", phase3_elapsed);
    if needs_baseline {
        let bl_status = if baseline_file.is_some() { "Built" } else { "Failed" };
        println!("  \u{2551}  Baseline:         {:<25}\u{2551}", bl_status);
    }
    println!("  \u{2560}{}\u{2563}", "\u{2550}".repeat(46));
    println!("  \u{2551}  Results:  {:<35}\u{2551}", output_dir.display());
    println!("  \u{2551}  Logs:     {:<35}\u{2551}", log_dir.display());
    if needs_baseline {
        println!("  \u{2551}  Baseline: {:<35}\u{2551}", baseline_dir.display());
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
