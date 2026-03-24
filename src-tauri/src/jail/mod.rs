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

use std::path::PathBuf;
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
    pub fn create(config: JailConfig) -> Result<Self, JailError> {
        let id = uuid::Uuid::new_v4().to_string();
        let jail_base = config
            .jail_base
            .unwrap_or_else(|| std::env::temp_dir().join("braid").join("jails"));
        let dest = jail_base.join(&id);
        std::fs::create_dir_all(&dest)?;

        // Create the mutation tracker with .gitignore support.
        let tracker = MutationTracker::new(config.project_path.clone()).shared();

        let cow_backend = cow::platform_cow();
        eprintln!(
            "[jail] creating {} jail at {}",
            cow_backend.name(),
            dest.display()
        );

        let mount = cow_backend.create(&config.project_path, &dest, tracker.clone())?;
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

    /// Tear down the jail. Unmounts the filesystem, removes session symlink, cleans up files.
    pub fn teardown(self) -> Result<(), JailError> {
        session_link::remove_session_link(&self.root)?;
        self._mount.teardown()?;
        Ok(())
    }
}
