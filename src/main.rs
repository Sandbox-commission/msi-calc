mod checkpoint;
mod config;
mod pipeline;
mod tui;

use log::info;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use config::VERSION;

pub(crate) static CANCELLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_cancelled() -> bool {
    CANCELLED.load(Ordering::Relaxed)
}

// ─── Interactive prompt ─────────────────────────────────────────────────────

fn prompt_for_pairs(config_pairs: PathBuf) -> Option<PathBuf> {
    let user_specified = std::env::args().any(|a| a == "-p" || a == "--pairs");
    if user_specified {
        if !config_pairs.exists() {
            eprintln!("File not found: {}", config_pairs.display());
            return None;
        }
        return Some(config_pairs);
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    let candidate = cwd.join("sample_pairs.csv");
    let found = candidate.exists();

    if found {
        eprintln!(
            "Found sample_pairs.csv in current directory: {}",
            cwd.display()
        );
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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .format_target(false)
        .init();

    let config = match config::parse_args() {
        Some(c) => c,
        None => return ExitCode::SUCCESS,
    };

    info!(
        "msi-calc v{} | Run with -h or --help for usage information",
        VERSION
    );
    info!(
        "Resources: {} job(s) x {} thread(s)/job = {} total cores{}",
        config.parallel_jobs,
        config.threads_per_job,
        config.parallel_jobs * config.threads_per_job as usize,
        if config.resources_auto {
            " [auto-detected]"
        } else {
            " [manual]"
        }
    );

    let pairs_file = match prompt_for_pairs(config.pairs_file.clone()) {
        Some(p) => p,
        None => return ExitCode::FAILURE,
    };

    pipeline::run(config, pairs_file)
}
