//! Platform-specific Copy-on-Write filesystem backends.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::jail::error::JailError;
use crate::jail::vcs::MutationTracker;

/// A live CoW mount. Dropping this tears down the mount.
pub struct CowMount {
    /// The directory Claude should use as CWD.
    pub root: PathBuf,
    /// The base jail directory (parent of root, upper, etc.).
    jail_dir: PathBuf,
    /// Platform-specific teardown. Called on drop.
    teardown_fn: Option<Box<dyn FnOnce() + Send>>,
}

impl CowMount {
    pub fn new(root: PathBuf, jail_dir: PathBuf, teardown_fn: impl FnOnce() + Send + 'static) -> Self {
        Self {
            root,
            jail_dir,
            teardown_fn: Some(Box::new(teardown_fn)),
        }
    }

    /// Explicitly tear down the mount and clean up files.
    pub fn teardown(mut self) -> Result<(), JailError> {
        // Run the platform-specific teardown (e.g. unmount WinFsp, fusermount -u).
        if let Some(f) = self.teardown_fn.take() {
            f();
        }
        // Clean up the jail directory.
        if self.jail_dir.exists() {
            std::fs::remove_dir_all(&self.jail_dir)
                .map_err(|e| JailError::CowSetup(format!("cleanup: {e}")))?;
        }
        Ok(())
    }
}

impl Drop for CowMount {
    fn drop(&mut self) {
        // Ensure unmount happens even if teardown() wasn't called explicitly.
        if let Some(f) = self.teardown_fn.take() {
            f();
        }
        // Best-effort cleanup of files.
        let _ = std::fs::remove_dir_all(&self.jail_dir);
    }
}

/// A CoW backend that creates isolated working directories.
pub trait CowLayer: Send + Sync {
    /// Create a CoW copy of `source` at `dest`. Returns a live mount handle.
    /// The `tracker` is used by backends that intercept writes (e.g. WinFsp)
    /// to record mutations in real-time.
    fn create(&self, source: &Path, dest: &Path, tracker: Arc<MutationTracker>) -> Result<CowMount, JailError>;

    /// Human-readable backend name.
    fn name(&self) -> &'static str;
}

/// Select the best CoW backend for the current platform.
pub fn platform_cow() -> Box<dyn CowLayer> {
    #[cfg(target_os = "linux")]
    {
        Box::new(OverlayFsCow)
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(ApfsCow)
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(super::winfsp_overlay::WinFspCow::new())
    }
}

// ─── Linux: fuse-overlayfs (unprivileged) ────────────────────────────────────

#[cfg(target_os = "linux")]
struct OverlayFsCow;

#[cfg(target_os = "linux")]
impl CowLayer for OverlayFsCow {
    fn name(&self) -> &'static str {
        "fuse-overlayfs"
    }

    fn create(&self, source: &Path, dest: &Path, _tracker: Arc<MutationTracker>) -> Result<CowMount, JailError> {
        let upper = dest.join(".cow_upper");
        let work = dest.join(".cow_work");
        let merged = dest.join("merged");

        std::fs::create_dir_all(&upper)?;
        std::fs::create_dir_all(&work)?;
        std::fs::create_dir_all(&merged)?;

        let status = std::process::Command::new("fuse-overlayfs")
            .arg("-o")
            .arg(format!(
                "lowerdir={},upperdir={},workdir={}",
                source.display(),
                upper.display(),
                work.display(),
            ))
            .arg(&merged)
            .status()
            .map_err(|e| JailError::CowSetup(format!("fuse-overlayfs: {e}")))?;

        if !status.success() {
            return Err(JailError::CowSetup(format!(
                "fuse-overlayfs exited with {status}"
            )));
        }

        let unmount_path = merged.clone();
        Ok(CowMount::new(merged, dest.to_path_buf(), move || {
            let _ = std::process::Command::new("fusermount")
                .arg("-u")
                .arg(&unmount_path)
                .status();
        }))
    }
}

// ─── macOS: APFS clone ───────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
struct ApfsCow;

#[cfg(target_os = "macos")]
impl CowLayer for ApfsCow {
    fn name(&self) -> &'static str {
        "apfs-clone"
    }

    fn create(&self, source: &Path, dest: &Path, _tracker: Arc<MutationTracker>) -> Result<CowMount, JailError> {
        let clone_dir = dest.join("clone");

        // cp -c -R uses APFS clonefile() for true CoW.
        let status = std::process::Command::new("cp")
            .args(["-c", "-R"])
            .arg(source)
            .arg(&clone_dir)
            .status()
            .map_err(|e| JailError::CowSetup(format!("cp -c: {e}")))?;

        if !status.success() {
            // Not APFS — fall back to regular copy.
            let status = std::process::Command::new("cp")
                .args(["-R"])
                .arg(source)
                .arg(&clone_dir)
                .status()
                .map_err(|e| JailError::CowSetup(format!("cp -R: {e}")))?;
            if !status.success() {
                return Err(JailError::CowSetup(format!("cp -R exited with {status}")));
            }
        }

        // No mount to unmount — just a directory clone.
        Ok(CowMount::new(clone_dir, dest.to_path_buf(), || {}))
    }
}
