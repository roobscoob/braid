//! Jail: isolated working directories for Claude Code.
//!
//! Each conversation turn runs Claude inside a jail — a CoW copy of the
//! project directory. After the turn completes, the changes are committed
//! to an in-memory git object store, creating a commit that corresponds
//! to the conversation node.
//!
//! The conversation tree IS the commit tree.

pub mod cow;
pub mod error;
pub mod session_link;
pub mod vcs;
#[cfg(target_os = "windows")]
pub mod winfsp_overlay;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use error::JailError;
use vcs::{DiffEntry, MutationTracker, VcsStore};

/// Configuration for creating a jail.
pub struct JailConfig {
    /// The real project directory to jail.
    pub project_path: PathBuf,
    /// Base directory for jail storage (default: system temp dir).
    pub jail_base: Option<PathBuf>,
    /// Shared VCS store (lives across jails).
    pub vcs: Arc<VcsStore>,
    /// Parent commit to restore from (for branching or continuing).
    /// If set, the committed files are materialized into the upper layer,
    /// and if `branch_from_jail` is set, ignored files are copied too.
    pub parent_commit: Option<gix::ObjectId>,
    /// Existing jail directory to copy ignored files from (for branching).
    /// Only used together with `parent_commit`.
    pub branch_from_jail: Option<PathBuf>,
}

/// A live jail instance. Created before spawning Claude, committed after.
pub struct Jail {
    /// The directory Claude should use as CWD.
    pub root: PathBuf,
    /// The shared in-memory VCS store.
    pub vcs: Arc<VcsStore>,
    /// Tracks file mutations, shared with the CoW layer.
    pub tracker: Arc<MutationTracker>,
    /// The live CoW mount. Dropping this unmounts the filesystem.
    _mount: cow::CowMount,
    /// Unique ID for this jail.
    id: String,
}

impl Jail {
    /// Create a new jail from a project directory.
    ///
    /// If `parent_commit` is set, the committed files are materialized into
    /// the upper layer so Claude sees the cumulative state. If `branch_from_jail`
    /// is also set, ignored files are copied from that jail's upper dir.
    pub fn create(config: JailConfig) -> Result<Self, JailError> {
        let id = uuid::Uuid::new_v4().to_string();
        let jail_base = config
            .jail_base
            .unwrap_or_else(|| std::env::temp_dir().join("braid").join("jails"));
        let dest = jail_base.join(&id);
        std::fs::create_dir_all(&dest)?;

        // If branching from an existing jail, copy its upper dir to seed
        // the new jail with ignored files (node_modules, .env, etc.).
        if let Some(ref source_jail) = config.branch_from_jail {
            let source_upper = source_jail.join("upper");
            let dest_upper = dest.join("upper");
            if source_upper.exists() {
                copy_dir_recursive(&source_upper, &dest_upper)?;
            }
        }

        // Materialize committed files from the parent commit into the upper dir.
        // Returns paths deleted relative to HEAD — these become initial whiteouts.
        let initial_whiteouts = if let Some(parent_oid) = config.parent_commit {
            let dest_upper = dest.join("upper");
            std::fs::create_dir_all(&dest_upper)?;
            config.vcs.materialize_commit(parent_oid, &dest_upper)?
        } else {
            Vec::new()
        };

        // Create the mutation tracker with .gitignore support.
        let tracker = MutationTracker::new(config.project_path.clone()).shared();

        let cow_backend = cow::platform_cow();
        eprintln!(
            "[jail] creating {} jail at {}",
            cow_backend.name(),
            dest.display()
        );

        let mount = cow_backend.create(&config.project_path, &dest, tracker.clone(), initial_whiteouts)?;
        let root = mount.root.clone();

        // Now that the mount is live, tell the tracker to read .gitignore
        // from the jail (not the source) on subsequent reloads.
        tracker.set_jail_root(root.clone());

        // Set up session symlink so --resume works.
        session_link::create_session_link(&root, &config.project_path)?;

        Ok(Jail {
            root,
            vcs: config.vcs,
            tracker,
            _mount: mount,
            id,
        })
    }

    /// Commit mutations on top of a parent commit.
    ///
    /// Reads the parent's tree, applies tracked mutations, creates a new
    /// in-memory commit. Returns the new commit's OID.
    pub fn commit(
        &self,
        message: &str,
        parent: gix::ObjectId,
    ) -> Result<gix::ObjectId, JailError> {
        let mutations = self.tracker.mutations();
        self.vcs
            .commit_mutations(&self.root, parent, &mutations, message)
    }

    /// Diff two commits.
    pub fn diff(
        &self,
        from: gix::ObjectId,
        to: gix::ObjectId,
    ) -> Result<Vec<DiffEntry>, JailError> {
        self.vcs.diff(from, to)
    }

    /// Get the jail's unique ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the jail's base directory (contains upper/, mount/, etc.).
    /// Used when branching to copy the upper dir.
    pub fn jail_dir(&self) -> &Path {
        &self._mount.jail_dir
    }

    /// Tear down the jail. Unmounts the filesystem but preserves the
    /// jail directory (upper layer) for future restoration.
    pub fn teardown(self) -> Result<(), JailError> {
        session_link::remove_session_link(&self.root)?;
        self._mount.teardown()?;
        Ok(())
    }
}

/// Recursively copy a directory tree.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), JailError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}
