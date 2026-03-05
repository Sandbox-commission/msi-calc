use std::fs::{self, File};
use std::path::{Path, PathBuf};

/// Path to the checkpoints directory inside output_dir
pub(crate) fn checkpoint_dir(output_dir: &Path) -> PathBuf {
    output_dir.join(".checkpoints")
}

/// Write a completion checkpoint for a sample
pub(crate) fn write_checkpoint(output_dir: &Path, name: &str) {
    let dir = checkpoint_dir(output_dir);
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.done"));
    let _ = File::create(&path);
}

/// Remove a checkpoint (on cancel/failure)
pub(crate) fn remove_checkpoint(output_dir: &Path, name: &str) {
    let path = checkpoint_dir(output_dir).join(format!("{name}.done"));
    let _ = fs::remove_file(&path);
}

/// Check if a sample completed successfully (checkpoint exists)
pub(crate) fn sample_is_done(output_dir: &Path, tumor_bam: &Path) -> bool {
    let name = crate::config::sample_name(tumor_bam);
    checkpoint_dir(output_dir)
        .join(format!("{name}.done"))
        .exists()
}

/// Check if a normal BAM has been profiled (checkpoint exists)
pub(crate) fn normal_is_profiled(output_dir: &Path, normal_bam: &Path) -> bool {
    let name = crate::config::sample_name(normal_bam);
    checkpoint_dir(output_dir)
        .join(format!("{name}_profile.done"))
        .exists()
}

/// Write a profiling checkpoint for a normal sample
pub(crate) fn write_profile_checkpoint(output_dir: &Path, name: &str) {
    let dir = checkpoint_dir(output_dir);
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}_profile.done"));
    let _ = File::create(&path);
}

/// Remove a profiling checkpoint
pub(crate) fn remove_profile_checkpoint(output_dir: &Path, name: &str) {
    let path = checkpoint_dir(output_dir).join(format!("{name}_profile.done"));
    let _ = fs::remove_file(&path);
}
