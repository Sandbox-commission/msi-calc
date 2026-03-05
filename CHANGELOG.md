# Changelog

## [0.3.0] - 2026-03-05

### Added
- **Modular architecture**: Split monolithic `main.rs` into `config.rs`, `pipeline.rs`, `checkpoint.rs`, `tui.rs`
- **Config file support**: `~/.config/msi-calc/config.json` with three-tier resolution (CLI > file > defaults)
- **Auto-detection**: CPU count for `--jobs` default via `std::thread::available_parallelism()`
- **Version flag**: `-V` / `--version` prints version and exits
- **Structured logging**: `log` + `env_logger` crate integration
- **CI/CD**: GitHub Actions workflows for build, clippy, fmt, Docker, and tagged releases
- **Three-tier setup.sh**: Docker / Mamba / Manual installation paths
- **Docker improvements**: dependency caching (dummy src trick), mambaorg/micromamba runtime, entrypoint script
- **Documentation**: CHANGELOG.md, INSTALLATION_GUIDE.md, QUICK_REFERENCE.md
- **Citation**: msisensor-pro reference with PMID in help text

### Changed
- Version synced to 0.3.0 (matches binary output)
- `Cargo.toml`: added `description`, `license`, `[[bin]]` section, new dependencies
- Uses `env!("CARGO_PKG_VERSION")` instead of hardcoded version strings

## [0.1.0] - 2024-01-01

### Added
- Initial release
- Parallel msisensor-pro execution with full-screen TUI
- Resume support with checkpoint files
- Mixed paired + tumor-only sample support with automatic baseline building
- Ctrl+C graceful cancellation with partial output cleanup
