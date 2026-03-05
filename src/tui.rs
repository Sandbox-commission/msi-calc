use crossterm::{
    cursor, event, execute,
    style::{self, Attribute, Color, Stylize},
    terminal::{self, ClearType},
};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{REFRESH_INTERVAL, VERSION};

// ─── Shared progress state ─────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) struct JobSlot {
    pub sample: String,
    pub started: Instant,
}

pub(crate) struct ProgressState {
    pub total: usize,
    pub completed: AtomicUsize,
    pub skipped: AtomicUsize,
    pub failed: AtomicUsize,
    pub active_jobs: Mutex<Vec<Option<JobSlot>>>,
    pub recent_events: Mutex<Vec<String>>,
    pub phase_label: Mutex<String>,
    pub start_time: Instant,
    pub completed_durations: Mutex<Vec<f64>>,
}

impl ProgressState {
    pub fn new(total: usize, parallel_jobs: usize, phase: &str) -> Self {
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

    pub fn set_active(&self, slot: usize, sample: &str) {
        if let Ok(mut jobs) = self.active_jobs.lock() {
            if slot < jobs.len() {
                jobs[slot] = Some(JobSlot {
                    sample: sample.to_string(),
                    started: Instant::now(),
                });
            }
        }
    }

    pub fn clear_slot(&self, slot: usize) {
        if let Ok(mut jobs) = self.active_jobs.lock() {
            if slot < jobs.len() {
                jobs[slot] = None;
            }
        }
    }

    pub fn add_event(&self, msg: String) {
        if let Ok(mut events) = self.recent_events.lock() {
            events.push(msg);
            if events.len() > 100 {
                events.remove(0);
            }
        }
    }

    pub fn done_count(&self) -> usize {
        self.completed.load(Ordering::Relaxed)
            + self.skipped.load(Ordering::Relaxed)
            + self.failed.load(Ordering::Relaxed)
    }

    pub fn record_duration(&self, secs: f64) {
        if let Ok(mut durations) = self.completed_durations.lock() {
            durations.push(secs);
        }
    }

    pub fn avg_duration(&self) -> f64 {
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
        self.phase_label
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default()
    }
}

// ─── Time formatting ────────────────────────────────────────────────────────

pub(crate) fn fmt_duration(d: Duration) -> String {
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

// ─── TUI rendering ─────────────────────────────────────────────────────────

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
    let filled = if total > 0 {
        bar_width * done / total
    } else {
        0
    };
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

    let _ = execute!(
        stdout,
        cursor::MoveTo(0, 0),
        terminal::Clear(ClearType::All)
    );

    let border_top = format!("\u{2554}{}\u{2557}", "\u{2550}".repeat(w.saturating_sub(2)));
    let border_mid = format!("\u{2560}{}\u{2563}", "\u{2550}".repeat(w.saturating_sub(2)));
    let border_bot = format!("\u{255A}{}\u{255D}", "\u{2550}".repeat(w.saturating_sub(2)));

    let mut row: u16 = 0;

    macro_rules! bline {
        ($color:expr, $text:expr) => {{
            let _ = execute!(
                stdout,
                cursor::MoveTo(0, row),
                style::SetForegroundColor(Color::Cyan)
            );
            let txt: String = $text;
            let txt_len = txt.len().min(w.saturating_sub(2));
            print!("\u{2551}");
            let _ = execute!(stdout, style::SetForegroundColor($color));
            print!("{}", &txt[..txt_len]);
            let _ = execute!(stdout, style::SetForegroundColor(Color::Cyan));
            print!(
                "{}\u{2551}",
                " ".repeat(w.saturating_sub(2).saturating_sub(txt_len))
            );
            row += 1;
        }};
    }

    macro_rules! separator {
        () => {{
            let _ = execute!(
                stdout,
                cursor::MoveTo(0, row),
                style::SetForegroundColor(Color::Cyan)
            );
            print!("{border_mid}");
            row += 1;
        }};
    }

    // Top border
    let _ = execute!(
        stdout,
        cursor::MoveTo(0, row),
        style::SetForegroundColor(Color::Cyan)
    );
    print!("{border_top}");
    row += 1;

    // Header
    let title = format!("MSI-CALC v{VERSION}");
    let subtitle = "Microsatellite Instability Analysis | msisensor-pro";
    let title_len = title.len();
    let pad_title = (w.saturating_sub(2).saturating_sub(title_len)) / 2;
    let pad_sub = (w.saturating_sub(2).saturating_sub(subtitle.len())) / 2;

    let _ = execute!(stdout, cursor::MoveTo(0, row));
    print!(
        "\u{2551}{}{}{}\u{2551}",
        " ".repeat(pad_title),
        title.with(Color::White).attribute(Attribute::Bold),
        " ".repeat(
            w.saturating_sub(2)
                .saturating_sub(pad_title)
                .saturating_sub(title_len)
        )
    );
    row += 1;

    let _ = execute!(
        stdout,
        cursor::MoveTo(0, row),
        style::SetForegroundColor(Color::Cyan)
    );
    print!(
        "\u{2551}{}{}{}\u{2551}",
        " ".repeat(pad_sub),
        subtitle.with(Color::DarkGrey),
        " ".repeat(
            w.saturating_sub(2)
                .saturating_sub(pad_sub)
                .saturating_sub(subtitle.len())
        )
    );
    row += 1;

    separator!();

    // Phase indicator
    let phase_line = format!("  {phase}");
    bline!(Color::Magenta, phase_line);

    // Show resume info if applicable
    if resumed > 0 {
        let resume_line =
            format!("  Resumed: {resumed} sample(s) already completed from previous run");
        bline!(Color::DarkYellow, resume_line);
    }

    separator!();

    // Overall progress bar
    bline!(Color::White, "  OVERALL PROGRESS".to_string());

    let _ = execute!(
        stdout,
        cursor::MoveTo(0, row),
        style::SetForegroundColor(Color::Cyan)
    );
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
    print!(
        "{}\u{2551}",
        " ".repeat(w.saturating_sub(2).saturating_sub(used))
    );
    row += 1;

    let stats_line = format!(
        "  {}/{} done   Elapsed: {}   ETA: {}   Speed: {:.1}/min",
        done,
        total,
        fmt_duration(elapsed),
        fmt_duration(eta),
        speed
    );
    let stats_len = stats_line.len();
    let _ = execute!(
        stdout,
        cursor::MoveTo(0, row),
        style::SetForegroundColor(Color::Cyan)
    );
    print!(
        "\u{2551}{}{}\u{2551}",
        stats_line.with(Color::White),
        " ".repeat(w.saturating_sub(2).saturating_sub(stats_len))
    );
    row += 1;

    separator!();

    // Active jobs
    let active_label = format!("  ACTIVE JOBS ({}/{})", active_count, parallel_jobs);
    let active_label_len = active_label.len();
    let _ = execute!(
        stdout,
        cursor::MoveTo(0, row),
        style::SetForegroundColor(Color::Cyan)
    );
    print!(
        "\u{2551}{}{}\u{2551}",
        active_label.with(Color::Yellow),
        " ".repeat(w.saturating_sub(2).saturating_sub(active_label_len))
    );
    row += 1;

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
                let _ = execute!(
                    stdout,
                    cursor::MoveTo(0, row),
                    style::SetForegroundColor(Color::Cyan)
                );
                print!(
                    "\u{2551}{}{}\u{2551}",
                    more.with(Color::DarkGrey),
                    " ".repeat(w.saturating_sub(2).saturating_sub(more_len))
                );
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
        let _ = execute!(
            stdout,
            cursor::MoveTo(0, row),
            style::SetForegroundColor(Color::Cyan)
        );
        print!("\u{2551}");
        let _ = execute!(stdout, style::SetForegroundColor(Color::White));
        print!("{display}");
        let _ = execute!(stdout, style::SetForegroundColor(Color::Cyan));
        print!(
            "{}\u{2551}",
            " ".repeat(w.saturating_sub(2).saturating_sub(display.len()))
        );
        row += 1;

        // Row 2: per-sample progress bar
        let _ = execute!(
            stdout,
            cursor::MoveTo(0, row),
            style::SetForegroundColor(Color::Cyan)
        );
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
            print!(
                "{}\u{2551}",
                " ".repeat(w.saturating_sub(2).saturating_sub(bar_used))
            );
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
            print!(
                "{}\u{2551}",
                " ".repeat(w.saturating_sub(2).saturating_sub(bar_used))
            );
        }
        row += 1;
    }

    separator!();

    // Counters
    let counters = format!(
        "  Completed: {}   Skipped: {}   Failed: {}   Remaining: {}",
        completed, skipped, failed, remaining
    );
    let _ = execute!(
        stdout,
        cursor::MoveTo(0, row),
        style::SetForegroundColor(Color::Cyan)
    );
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
    print!(
        "{}\u{2551}",
        " ".repeat(w.saturating_sub(2).saturating_sub(used_len))
    );
    row += 1;

    separator!();

    // Recent activity
    let log_label = "  RECENT ACTIVITY";
    let _ = execute!(
        stdout,
        cursor::MoveTo(0, row),
        style::SetForegroundColor(Color::Cyan)
    );
    print!(
        "\u{2551}{}{}\u{2551}",
        log_label.with(Color::Magenta),
        " ".repeat(w.saturating_sub(2).saturating_sub(log_label.len()))
    );
    row += 1;

    let max_event_rows = h.saturating_sub(row as usize + 3);
    let start = events.len().saturating_sub(max_event_rows);
    for event in &events[start..] {
        let _ = execute!(
            stdout,
            cursor::MoveTo(0, row),
            style::SetForegroundColor(Color::Cyan)
        );
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
        print!(
            "{}\u{2551}",
            " ".repeat(w.saturating_sub(2).saturating_sub(ev.len()))
        );
        row += 1;
    }

    // Fill remaining
    while (row as usize) < h.saturating_sub(2) {
        let _ = execute!(
            stdout,
            cursor::MoveTo(0, row),
            style::SetForegroundColor(Color::Cyan)
        );
        print!("\u{2551}{}\u{2551}", " ".repeat(w.saturating_sub(2)));
        row += 1;
    }

    // Footer
    let _ = execute!(
        stdout,
        cursor::MoveTo(0, row),
        style::SetForegroundColor(Color::Cyan)
    );
    let cancelled = crate::is_cancelled();
    let cancel_hint = if cancelled {
        "  CANCELLING..."
    } else {
        "  Ctrl+C to cancel"
    };
    let timestamp = format!("Updated: {} ", chrono::Local::now().format("%H:%M:%S"));
    let footer_pad = w
        .saturating_sub(2)
        .saturating_sub(cancel_hint.len())
        .saturating_sub(timestamp.len());
    if cancelled {
        print!(
            "\u{2551}{}{}{}\u{2551}",
            cancel_hint.with(Color::Red),
            " ".repeat(footer_pad),
            timestamp.with(Color::DarkGrey)
        );
    } else {
        print!(
            "\u{2551}{}{}{}\u{2551}",
            cancel_hint.with(Color::DarkGrey),
            " ".repeat(footer_pad),
            timestamp.with(Color::DarkGrey)
        );
    }
    row += 1;

    let _ = execute!(
        stdout,
        cursor::MoveTo(0, row),
        style::SetForegroundColor(Color::Cyan)
    );
    print!("{border_bot}");

    let _ = execute!(stdout, style::ResetColor);
    let _ = stdout.flush();
}

// ─── Display thread control ─────────────────────────────────────────────────

pub(crate) struct DisplayThread {
    flag: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl DisplayThread {
    pub fn start(state: Arc<ProgressState>, parallel_jobs: usize, resumed: usize) -> Self {
        let flag = Arc::new(AtomicBool::new(false));
        let display_flag = Arc::clone(&flag);

        let handle = std::thread::spawn(move || {
            let mut out = io::stdout();
            loop {
                render_screen(&mut out, &state, parallel_jobs, resumed);

                if event::poll(REFRESH_INTERVAL).unwrap_or(false) {
                    if let Ok(event::Event::Key(key)) = event::read() {
                        if key.code == event::KeyCode::Char('c')
                            && key.modifiers.contains(event::KeyModifiers::CONTROL)
                        {
                            crate::CANCELLED.store(true, Ordering::Relaxed);
                        }
                    }
                }

                if display_flag.load(Ordering::Relaxed) || crate::is_cancelled() {
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

    pub fn stop(&mut self) {
        self.flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
