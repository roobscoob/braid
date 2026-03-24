//! Platform-specific Copy-on-Write filesystem backends.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::jail::error::JailError;
use crate::jail::vcs::MutationTracker;

// ─── Global mount registry ───────────────────────────────────────────────────

/// Global registry of teardown hooks for live mounts. Each entry is a
/// one-shot closure that stops the filesystem host and unmounts.
static MOUNT_REGISTRY: OnceLock<Mutex<Vec<(usize, Box<dyn FnOnce() + Send>)>>> = OnceLock::new();

fn registry() -> &'static Mutex<Vec<(usize, Box<dyn FnOnce() + Send>)>> {
    MOUNT_REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Unique ID counter for registry entries.
static NEXT_MOUNT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn register_mount(teardown: impl FnOnce() + Send + 'static) -> usize {
    let id = NEXT_MOUNT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    registry().lock().unwrap().push((id, Box::new(teardown)));
    id
}

fn deregister_mount(id: usize) {
    registry().lock().unwrap().retain(|(i, _)| *i != id);
}

/// Unmount all live filesystems. Called from Ctrl+C handler and panic hook.
/// Safe to call multiple times — each hook runs at most once.
pub fn shutdown_all_mounts() {
    let hooks: Vec<(usize, Box<dyn FnOnce() + Send>)> =
        registry().lock().unwrap().drain(..).collect();
    for (_, hook) in hooks {
        hook();
    }
}

/// A live CoW mount. Dropping this unmounts the filesystem but
/// preserves the jail directory (upper layer) for future restoration.
pub struct CowMount {
    /// The directory Claude should use as CWD.
    pub root: PathBuf,
    /// The base jail directory (parent of root, upper, etc.).
    pub jail_dir: PathBuf,
    /// Platform-specific teardown. Called on drop.
    teardown_fn: Option<Box<dyn FnOnce() + Send>>,
    /// Registry ID for the global shutdown hook.
    registry_id: Option<usize>,
}

impl CowMount {
    pub fn new(root: PathBuf, jail_dir: PathBuf, teardown_fn: impl FnOnce() + Send + 'static) -> Self {
        // Wrap the teardown in an Arc<Mutex<Option<...>>> so both Drop and
        // the global shutdown registry can race to run it (exactly once).
        let shared: Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>> =
            Arc::new(Mutex::new(Some(Box::new(teardown_fn))));

        let shared_for_registry = shared.clone();
        let registry_id = register_mount(move || {
            if let Some(f) = shared_for_registry.lock().unwrap().take() {
                f();
            }
        });

        Self {
            root,
            jail_dir,
            teardown_fn: Some(Box::new(move || {
                if let Some(f) = shared.lock().unwrap().take() {
                    f();
                }
            })),
            registry_id: Some(registry_id),
        }
    }

    /// Unmount the filesystem. The jail directory (upper layer) is preserved
    /// for future restoration.
    pub fn teardown(mut self) -> Result<(), JailError> {
        if let Some(id) = self.registry_id.take() {
            deregister_mount(id);
        }
        if let Some(f) = self.teardown_fn.take() {
            f();
        }
        // Remove the mount point (reparse point / symlink) but keep the jail dir.
        let mount = &self.root;
        if mount.exists() {
            let _ = std::fs::remove_dir(mount);
        }
        Ok(())
    }
}

impl Drop for CowMount {
    fn drop(&mut self) {
        if let Some(id) = self.registry_id.take() {
            deregister_mount(id);
        }
        // Unmount only — preserve the jail directory.
        if let Some(f) = self.teardown_fn.take() {
            f();
        }
        let mount = &self.root;
        if mount.exists() {
            let _ = std::fs::remove_dir(mount);
        }
    }
}

/// A CoW backend that creates isolated working directories.
pub trait CowLayer: Send + Sync {
    /// Create a CoW copy of `source` at `dest`. Returns a live mount handle.
    /// The `tracker` is used by backends that intercept writes (e.g. WinFsp)
    /// to record mutations in real-time. `initial_whiteouts` are paths that
    /// should appear deleted in the overlay (from a previous commit's deletions).
    fn create(
        &self,
        source: &Path,
        dest: &Path,
        tracker: Arc<MutationTracker>,
        initial_whiteouts: Vec<String>,
    ) -> Result<CowMount, JailError>;

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

    fn create(&self, source: &Path, dest: &Path, _tracker: Arc<MutationTracker>, _initial_whiteouts: Vec<String>) -> Result<CowMount, JailError> {
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

    fn create(&self, source: &Path, dest: &Path, _tracker: Arc<MutationTracker>, _initial_whiteouts: Vec<String>) -> Result<CowMount, JailError> {
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
