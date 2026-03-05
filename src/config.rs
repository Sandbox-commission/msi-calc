use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use log::{debug, info, warn};

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) const DEFAULT_PAIRS: &str = "sample_pairs.csv";
pub(crate) const DEFAULT_OUTPUT: &str = "msi-results";
pub(crate) const REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

// ─── Config file support ────────────────────────────────────────────────────
// Resolution order (highest to lowest priority):
// 1. CLI flags (--reference, --pairs, etc.)
// 2. ~/.config/msi-calc/config.json
// 3. Auto-detection (CPU count + RAM for jobs/threads)

#[derive(serde::Deserialize, Default)]
pub(crate) struct FileConfig {
    pub reference: Option<String>,
    pub pairs: Option<String>,
    #[allow(dead_code)]
    pub msisensor_env: Option<String>,
}

fn config_file_path() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| {
        let home = PathBuf::from(home);
        home.join(".config/msi-calc/config.json")
    })
}

fn load_file_config() -> FileConfig {
    match config_file_path() {
        None => FileConfig::default(),
        Some(path) => match fs::read_to_string(&path) {
            Ok(content) => {
                debug!("Loaded config from {}", path.display());
                match serde_json::from_str(&content) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        warn!(
                            "Failed to parse config file {}: {} — using defaults",
                            path.display(),
                            e
                        );
                        FileConfig::default()
                    }
                }
            }
            Err(_) => FileConfig::default(),
        },
    }
}

// ─── System resource detection ───────────────────────────────────────────────

/// Read available RAM from /proc/meminfo (Linux). Returns bytes, or 0 on failure.
pub(crate) fn read_available_ram() -> u64 {
    let Ok(file) = std::fs::File::open("/proc/meminfo") else {
        return 0;
    };
    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kib: u64 = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            return kib * 1024;
        }
    }
    0
}

/// Detect available system resources: (RAM in bytes, CPU count).
pub(crate) fn detect_system_resources() -> (u64, usize) {
    let ram = read_available_ram();
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    (ram, cpus)
}

/// Compute optimal (jobs, threads_per_job) from available resources.
///
/// msisensor-pro uses ~2 GB RAM per job (BAM I/O + microsatellite analysis).
/// We reserve 2 GB for OS/buffers and then fit as many jobs as possible
/// within both RAM and CPU constraints.
///
/// Returns (parallel_jobs, threads_per_job).
pub(crate) fn auto_config_resources(available_ram: u64, total_cpus: usize) -> (usize, u32) {
    const PER_JOB_RAM: u64 = 2_000_000_000; // ~2 GB per msisensor-pro job
    const OS_BUFFER: u64 = 2_000_000_000; // reserve 2 GB for OS

    let usable_ram = available_ram.saturating_sub(OS_BUFFER);

    // Max jobs by RAM
    let max_by_ram = (usable_ram / PER_JOB_RAM) as usize;

    // Max jobs by CPU (need at least 2 threads per job for msisensor-pro)
    let max_by_cpu = total_cpus / 2;

    // Take the tighter constraint, minimum 1
    let jobs = max_by_ram.min(max_by_cpu).max(1);

    // Distribute remaining CPUs across jobs
    let threads = (total_cpus / jobs).max(1) as u32;

    (jobs, threads)
}

pub(crate) fn config_dir_display() -> String {
    config_file_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~/.config/msi-calc/config.json".to_string())
}

// ─── Config struct ──────────────────────────────────────────────────────────

pub(crate) struct Config {
    pub pairs_file: PathBuf,
    pub output_dir: PathBuf,
    pub parallel_jobs: usize,
    pub threads_per_job: u32,
    pub reference_fasta: PathBuf,
    pub resources_auto: bool,
}

// ─── Help ───────────────────────────────────────────────────────────────────

pub(crate) fn usage() {
    let (avail_ram, total_cpus) = detect_system_resources();
    let (default_jobs, default_threads) = auto_config_resources(avail_ram, total_cpus);
    let ram_gb = avail_ram as f64 / 1_000_000_000.0;
    eprintln!("msi-calc v{VERSION}");
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
    eprintln!("                            [default: {DEFAULT_PAIRS}]");
    eprintln!("    -o, --output <DIR>      Directory to write MSI results");
    eprintln!("                            [default: {DEFAULT_OUTPUT}]");
    eprintln!("    -j, --jobs <N>          Number of samples to process in parallel");
    eprintln!(
        "                            [default: {default_jobs} (auto: {total_cpus} CPUs, {ram_gb:.0} GB RAM)]"
    );
    eprintln!("    -t, --threads <N>       Threads allocated per msisensor-pro job");
    eprintln!(
        "                            [default: {default_threads} (auto: {total_cpus} CPUs / {default_jobs} jobs)]"
    );
    eprintln!("    -V, --version           Print version and exit");
    eprintln!("    -h, --help              Print this help message");
    eprintln!();
    eprintln!("CONFIG FILE:");
    eprintln!("    {}", config_dir_display());
    eprintln!();
    eprintln!("    Optional JSON config to set defaults (CLI flags override):");
    eprintln!("    {{");
    eprintln!("      \"reference\": \"/path/to/reference.fa\",");
    eprintln!("      \"pairs\": \"/path/to/sample_pairs.csv\",");
    eprintln!("      \"msisensor_env\": \"/path/to/conda/env\"");
    eprintln!("    }}");
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
    eprintln!(
        "    Total CPU usage = jobs x threads (auto: {default_jobs} x {default_threads} = {} cores).",
        default_jobs * default_threads as usize
    );
    eprintln!("    Resources auto-detected: {total_cpus} CPUs, {ram_gb:.1} GB RAM available.");
    eprintln!("    ~2 GB RAM reserved per job, 2 GB reserved for OS.");
    eprintln!("    Override with -j and -t if needed.");
    eprintln!("    Press Ctrl+C to gracefully stop (kills running processes).");
    eprintln!();
    eprintln!("CITATION:");
    eprintln!(
        "    msisensor-pro: Jia P et al. MSIsensor-pro: Fast, Accurate, and Matched-normal-free"
    );
    eprintln!(
        "    Detection of Microsatellite Instability. Genomics Proteomics Bioinformatics (2020)."
    );
    eprintln!("    PMID: 32171661");
}

// ─── Arg parsing ────────────────────────────────────────────────────────────

pub(crate) fn parse_args() -> Option<Config> {
    let file_cfg = load_file_config();

    let args: Vec<String> = env::args().collect();
    let mut pairs_file: Option<PathBuf> = None;
    let mut output_dir = PathBuf::from(DEFAULT_OUTPUT);
    let mut parallel_jobs: Option<usize> = None;
    let mut threads_per_job: Option<u32> = None;
    let mut reference_fasta: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                usage();
                return None;
            }
            "-V" | "--version" => {
                eprintln!("msi-calc v{VERSION}");
                return None;
            }
            "-r" | "--reference" => {
                i += 1;
                reference_fasta = Some(PathBuf::from(&args[i]));
            }
            "-p" | "--pairs" => {
                i += 1;
                pairs_file = Some(PathBuf::from(&args[i]));
            }
            "-o" | "--output" => {
                i += 1;
                output_dir = PathBuf::from(&args[i]);
            }
            "-j" | "--jobs" => {
                i += 1;
                parallel_jobs = Some(args[i].parse().expect("Invalid value for --jobs"));
            }
            "-t" | "--threads" => {
                i += 1;
                threads_per_job = Some(args[i].parse().expect("Invalid value for --threads"));
            }
            other => {
                eprintln!("Unknown option: {other}");
                usage();
                return None;
            }
        }
        i += 1;
    }

    // Resolve with config file fallbacks
    let reference_fasta = reference_fasta
        .or_else(|| file_cfg.reference.map(PathBuf::from))
        .unwrap_or_else(|| {
            eprintln!("Error: -r / --reference <FASTA> is required.");
            eprintln!("       Provide the path to your reference genome (.fa or .fasta).");
            eprintln!("       Or set \"reference\" in {}", config_dir_display());
            usage();
            std::process::exit(1);
        });

    let pairs_file = pairs_file
        .or_else(|| file_cfg.pairs.map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PAIRS));

    // Resolve resource parameters: auto-detect if not explicitly set
    let resources_auto = parallel_jobs.is_none() || threads_per_job.is_none();
    let (parallel_jobs, threads_per_job) = if resources_auto {
        let (avail_ram, total_cpus) = detect_system_resources();
        let (auto_jobs, auto_threads) = auto_config_resources(avail_ram, total_cpus);
        let ram_gb = avail_ram as f64 / 1_000_000_000.0;
        info!(
            "Auto-detected resources: {total_cpus} CPUs, {ram_gb:.1} GB RAM -> \
             {auto_jobs} jobs x {auto_threads} threads",
        );
        (
            parallel_jobs.unwrap_or(auto_jobs),
            threads_per_job.unwrap_or(auto_threads),
        )
    } else {
        (parallel_jobs.unwrap(), threads_per_job.unwrap())
    };

    if parallel_jobs == 0 {
        eprintln!("Error: --jobs must be >= 1");
        return None;
    }
    if threads_per_job == 0 {
        eprintln!("Error: --threads must be >= 1");
        return None;
    }

    Some(Config {
        pairs_file,
        output_dir,
        parallel_jobs,
        threads_per_job,
        reference_fasta,
        resources_auto,
    })
}

pub(crate) fn sample_name(bam: &Path) -> String {
    bam.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .trim_end_matches("_final.bam")
        .to_string()
}
