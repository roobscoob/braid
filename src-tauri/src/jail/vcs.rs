//! In-memory version control for jail snapshots.
//!
//! Uses the project's existing `.git` repo as a read-only baseline.
//! All new objects (blobs, trees, commits) are stored in memory.
//! The conversation tree maps directly to an in-memory commit graph.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use gix::objs::tree::{Entry as TreeEntry, EntryMode};
use gix::objs::{Tree, WriteTo};
use gix::ObjectId;

use crate::jail::error::JailError;

// ─── Mutation tracking ───────────────────────────────────────────────────────

/// The kind of mutation observed on a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationKind {
    Created,
    Modified,
    Deleted,
}

/// A recorded mutation.
#[derive(Debug, Clone)]
pub struct Mutation {
    /// Relative path (forward-slash separated).
    pub rel_path: String,
    pub kind: MutationKind,
}

/// Thread-safe mutation tracker with `.gitignore` support.
///
/// Shared between the CoW filesystem and the VCS. The CoW layer calls
/// `record_*` methods on every write/create/delete. The tracker checks
/// `.gitignore` and stores only non-ignored mutations.
pub struct MutationTracker {
    /// The original project root (for initial `.gitignore` load).
    source_root: PathBuf,
    /// The jail mount root (set after mount). Used to read `.gitignore`
    /// after Claude modifies it inside the jail.
    jail_root: RwLock<Option<PathBuf>>,
    /// Accumulated mutations, keyed by relative path.
    mutations: RwLock<BTreeMap<String, MutationKind>>,
    /// The ignore matcher, rebuilt when `.gitignore` changes.
    ignore: RwLock<gix::ignore::Search>,
}

impl MutationTracker {
    /// Create a new tracker, loading `.gitignore` from the source directory.
    pub fn new(source_root: PathBuf) -> Self {
        let ignore = Self::load_ignore(&source_root);
        Self {
            source_root,
            jail_root: RwLock::new(None),
            mutations: RwLock::new(BTreeMap::new()),
            ignore: RwLock::new(ignore),
        }
    }

    /// Load `.gitignore` patterns from the source root, including nested
    /// `.gitignore` files in subdirectories and `.git/info/exclude`.
    fn load_ignore(root: &Path) -> gix::ignore::Search {
        let mut search = gix::ignore::Search::default();

        // .git/info/exclude
        let exclude_path = root.join(".git").join("info").join("exclude");
        if exclude_path.exists() {
            if let Ok(bytes) = std::fs::read(&exclude_path) {
                search.add_patterns_buffer(&bytes, exclude_path, Some(root));
            }
        }

        // Walk the tree for .gitignore files (root + subdirectories).
        Self::load_gitignores_recursive(root, root, &mut search);

        search
    }

    /// Recursively load `.gitignore` files from `dir` and its children.
    fn load_gitignores_recursive(
        dir: &Path,
        root: &Path,
        search: &mut gix::ignore::Search,
    ) {
        let gitignore_path = dir.join(".gitignore");
        if gitignore_path.exists() {
            if let Ok(bytes) = std::fs::read(&gitignore_path) {
                search.add_patterns_buffer(&bytes, gitignore_path, Some(dir));
            }
        }

        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // Skip .git directory.
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name == ".git" {
                    continue;
                }
            }
            // Skip if this directory is already ignored.
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                let bstr: &bstr::BStr = rel_str.as_str().into();
                let is_ignored = search
                    .pattern_matching_relative_path(
                        bstr,
                        Some(true),
                        gix::glob::pattern::Case::Fold,
                    )
                    .map(|m| !m.pattern.is_negative())
                    .unwrap_or(false);
                if is_ignored {
                    continue;
                }
            }
            Self::load_gitignores_recursive(&path, root, search);
        }
    }

    /// Check if a relative path is ignored.
    pub fn is_ignored(&self, rel_path: &str, is_dir: bool) -> bool {
        // Always track .gitignore files themselves.
        if rel_path == ".gitignore" || rel_path.ends_with("/.gitignore") {
            return false;
        }
        // Always ignore .git.
        if rel_path == ".git" || rel_path.starts_with(".git/") {
            return true;
        }
        let ignore = self.ignore.read().unwrap();
        let bstr: &bstr::BStr = rel_path.into();
        ignore
            .pattern_matching_relative_path(
                bstr,
                Some(is_dir),
                gix::glob::pattern::Case::Fold,
            )
            .map(|m| !m.pattern.is_negative())
            .unwrap_or(false)
    }

    /// Record a file creation.
    pub fn record_create(&self, rel_path: &str, is_dir: bool) {
        if self.is_ignored(rel_path, is_dir) {
            return;
        }
        if is_dir {
            return; // Directories are implicit in git.
        }
        self.mutations
            .write()
            .unwrap()
            .insert(rel_path.to_string(), MutationKind::Created);

        // If a .gitignore was created, re-evaluate everything.
        if rel_path == ".gitignore" || rel_path.ends_with("/.gitignore") {
            self.reload_ignore();
        }
    }

    /// Record a file modification.
    pub fn record_modify(&self, rel_path: &str) {
        if self.is_ignored(rel_path, false) {
            return;
        }
        let mut mutations = self.mutations.write().unwrap();
        // Don't downgrade Created to Modified.
        mutations
            .entry(rel_path.to_string())
            .or_insert(MutationKind::Modified);

        // If a .gitignore was modified, re-evaluate everything.
        if rel_path == ".gitignore" || rel_path.ends_with("/.gitignore") {
            drop(mutations);
            self.reload_ignore();
        }
    }

    /// Record a file deletion.
    pub fn record_delete(&self, rel_path: &str) {
        if self.is_ignored(rel_path, false) {
            return;
        }
        self.mutations
            .write()
            .unwrap()
            .insert(rel_path.to_string(), MutationKind::Deleted);
    }

    /// Clear all recorded mutations. Call after committing so the next
    /// turn on the same jail starts with a clean slate.
    pub fn clear(&self) {
        self.mutations.write().unwrap().clear();
    }

    /// Record a rename (old path deleted, new path created).
    pub fn record_rename(&self, old_rel: &str, new_rel: &str, is_dir: bool) {
        self.record_delete(old_rel);
        self.record_create(new_rel, is_dir);
    }

    /// Reload `.gitignore` and re-check all tracked mutations.
    fn reload_ignore(&self) {
        // Prefer the jail root (where Claude may have modified .gitignore)
        // over the original source root.
        let root = self
            .jail_root
            .read()
            .unwrap()
            .clone()
            .unwrap_or_else(|| self.source_root.clone());
        let new_ignore = Self::load_ignore(&root);
        *self.ignore.write().unwrap() = new_ignore;

        // Re-evaluate: remove mutations that are now ignored.
        let ignore = self.ignore.read().unwrap();
        let mut mutations = self.mutations.write().unwrap();
        mutations.retain(|path, _| {
            if path == ".gitignore" || path.ends_with("/.gitignore") {
                return true;
            }
            let bstr: &bstr::BStr = path.as_str().into();
            !ignore
                .pattern_matching_relative_path(
                    bstr,
                    Some(false),
                    gix::glob::pattern::Case::Fold,
                )
                .map(|m| !m.pattern.is_negative())
                .unwrap_or(false)
        });
    }

    /// Get all recorded mutations.
    pub fn mutations(&self) -> Vec<Mutation> {
        self.mutations
            .read()
            .unwrap()
            .iter()
            .map(|(path, kind)| Mutation {
                rel_path: path.clone(),
                kind: kind.clone(),
            })
            .collect()
    }

    /// Set the jail root path. Call this after the CoW mount is ready
    /// so that `.gitignore` reloads read from the jail, not the source.
    pub fn set_jail_root(&self, root: PathBuf) {
        *self.jail_root.write().unwrap() = Some(root);
    }

    /// Get a shared reference wrapped in Arc for use by the CoW layer.
    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }
}

// ─── Diff types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DiffEntry {
    pub path: String,
    pub kind: DiffKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffKind {
    Added,
    Modified,
    Deleted,
}

// ─── In-memory VCS store ─────────────────────────────────────────────────────

/// In-memory git object store backed by the project's `.git` for reads.
///
/// Lives in Tauri app state, shared across all jails for a project.
/// New objects (blobs, trees, commits from jail mutations) are stored in memory.
/// The project's `.git` is used read-only to resolve the baseline tree.
pub struct VcsStore {
    /// Path to the project's `.git` directory.
    project_git: PathBuf,
    /// In-memory object store: OID → serialized object bytes.
    objects: RwLock<HashMap<ObjectId, Vec<u8>>>,
}

impl VcsStore {
    /// Create a new store for a project. The project must have a `.git` directory.
    pub fn open(project_path: &Path) -> Result<Arc<Self>, JailError> {
        let git_dir = project_path.join(".git");
        if !git_dir.exists() {
            return Err(JailError::Git(format!(
                "Not a git repository: {}",
                project_path.display()
            )));
        }

        Ok(Arc::new(Self {
            project_git: git_dir,
            objects: RwLock::new(HashMap::new()),
        }))
    }

    /// Get the current HEAD commit's tree OID from the project repo.
    /// This is the baseline for the first conversation message.
    pub fn head_commit_id(&self) -> Result<ObjectId, JailError> {
        let repo = gix::open(&self.project_git).map_err(|e| JailError::Git(e.to_string()))?;
        let head = repo
            .head_commit()
            .map_err(|e| JailError::Git(format!("HEAD: {e}")))?;
        Ok(head.id().detach())
    }

    /// Find an object — first check in-memory store, then fall back to the project repo.
    fn find_object(&self, id: ObjectId) -> Result<Vec<u8>, JailError> {
        // Check in-memory first.
        if let Some(data) = self.objects.read().unwrap().get(&id) {
            return Ok(data.clone());
        }

        // Fall back to the project's git repo.
        let repo = gix::open(&self.project_git).map_err(|e| JailError::Git(e.to_string()))?;
        let obj = repo
            .find_object(id)
            .map_err(|e| JailError::Git(e.to_string()))?;
        Ok(obj.data.to_vec())
    }

    /// Write an object to the in-memory store. Returns its OID.
    fn write_object_raw(&self, kind: gix::object::Kind, data: &[u8]) -> Result<ObjectId, JailError> {
        let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, kind, data)
            .map_err(|e| JailError::Git(format!("hash: {e}")))?;

        self.objects
            .write()
            .unwrap()
            .insert(id, data.to_vec());

        Ok(id)
    }

    /// Write a blob to the in-memory store.
    fn write_blob(&self, data: &[u8]) -> Result<ObjectId, JailError> {
        self.write_object_raw(gix::object::Kind::Blob, data)
    }

    /// Write a tree to the in-memory store.
    fn write_tree(&self, tree: &Tree) -> Result<ObjectId, JailError> {
        let mut buf = Vec::new();
        tree.write_to(&mut buf)
            .map_err(|e| JailError::Git(format!("serialize tree: {e}")))?;
        self.write_object_raw(gix::object::Kind::Tree, &buf)
    }

    /// Write a commit to the in-memory store.
    fn write_commit(&self, commit: &gix::objs::Commit) -> Result<ObjectId, JailError> {
        let mut buf = Vec::new();
        commit
            .write_to(&mut buf)
            .map_err(|e| JailError::Git(format!("serialize commit: {e}")))?;
        self.write_object_raw(gix::object::Kind::Commit, &buf)
    }

    /// Get the tree OID from a commit.
    fn commit_tree(&self, commit_id: ObjectId) -> Result<ObjectId, JailError> {
        let data = self.find_object(commit_id)?;
        let commit = gix::objs::CommitRef::from_bytes(&data)
            .map_err(|e| JailError::Git(format!("parse commit: {e}")))?;
        Ok(commit.tree())
    }

    /// Flatten a tree into a map of relative path → (blob OID, entry mode).
    fn flatten_tree(
        &self,
        tree_id: ObjectId,
        prefix: &str,
        out: &mut BTreeMap<String, (ObjectId, EntryMode)>,
    ) -> Result<(), JailError> {
        let data = self.find_object(tree_id)?;
        let tree_ref = gix::objs::TreeRef::from_bytes(&data)
            .map_err(|e| JailError::Git(format!("parse tree: {e}")))?;

        for entry in &tree_ref.entries {
            let name = entry.filename.to_string();
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };

            if entry.mode.is_tree() {
                self.flatten_tree(entry.oid.into(), &path, out)?;
            } else {
                out.insert(path, (entry.oid.into(), entry.mode));
            }
        }
        Ok(())
    }

    /// Build a nested tree from a flat list of (path, blob_oid, mode) tuples.
    fn build_tree_from_entries(
        &self,
        entries: &[(String, ObjectId, EntryMode)],
    ) -> Result<ObjectId, JailError> {
        let mut files: Vec<TreeEntry> = Vec::new();
        let mut subdirs: BTreeMap<String, Vec<(String, ObjectId, EntryMode)>> = BTreeMap::new();

        for (path, oid, mode) in entries {
            let parts: Vec<&str> = path.splitn(2, '/').collect();
            if parts.len() == 1 {
                files.push(TreeEntry {
                    mode: *mode,
                    filename: parts[0].into(),
                    oid: *oid,
                });
            } else {
                subdirs
                    .entry(parts[0].to_string())
                    .or_default()
                    .push((parts[1].to_string(), *oid, *mode));
            }
        }

        for (dir_name, children) in &subdirs {
            let subtree_id = self.build_tree_from_entries(children)?;
            files.push(TreeEntry {
                mode: gix::objs::tree::EntryKind::Tree.into(),
                filename: dir_name.as_str().into(),
                oid: subtree_id,
            });
        }

        files.sort();

        let tree = Tree { entries: files };
        self.write_tree(&tree)
    }

    /// Create a commit pointing to a tree with an optional parent.
    fn create_commit(
        &self,
        tree_id: ObjectId,
        parent: Option<ObjectId>,
        message: &str,
    ) -> Result<ObjectId, JailError> {
        let sig = gix::actor::Signature {
            name: "braid".into(),
            email: "braid@local".into(),
            time: gix::date::Time::now_local_or_utc(),
        };
        let mut time_buf = gix::date::parse::TimeBuf::default();
        let sig_ref = sig.to_ref(&mut time_buf);

        let parents: Vec<ObjectId> = parent.into_iter().collect();
        let commit = gix::objs::Commit {
            tree: tree_id,
            parents: parents.into(),
            author: sig_ref.into(),
            committer: sig_ref.into(),
            encoding: None,
            message: message.into(),
            extra_headers: Default::default(),
        };

        self.write_commit(&commit)
    }

    /// Commit mutations on top of a parent commit.
    ///
    /// Reads the parent's tree (from memory or `.git`), applies mutations
    /// by reading changed files from the jail, and creates a new in-memory commit.
    pub fn commit_mutations(
        &self,
        jail_root: &Path,
        parent: ObjectId,
        mutations: &[Mutation],
        message: &str,
    ) -> Result<ObjectId, JailError> {
        if mutations.is_empty() {
            return Ok(parent);
        }

        let parent_tree_id = self.commit_tree(parent)?;

        // Flatten the parent tree into a file map.
        let mut file_map = BTreeMap::new();
        self.flatten_tree(parent_tree_id, "", &mut file_map)?;

        // Apply mutations.
        for mutation in mutations {
            match &mutation.kind {
                MutationKind::Created | MutationKind::Modified => {
                    let full_path = jail_root.join(&mutation.rel_path);
                    if full_path.is_file() {
                        let data = std::fs::read(&full_path)?;
                        let blob_id = self.write_blob(&data)?;
                        // Preserve existing mode if the file was already tracked,
                        // otherwise default to regular blob.
                        let mode = file_map
                            .get(&mutation.rel_path)
                            .map(|(_, m)| *m)
                            .unwrap_or(gix::objs::tree::EntryKind::Blob.into());
                        file_map.insert(mutation.rel_path.clone(), (blob_id, mode));
                    }
                }
                MutationKind::Deleted => {
                    file_map.remove(&mutation.rel_path);
                }
            }
        }

        let entries: Vec<(String, ObjectId, EntryMode)> = file_map
            .into_iter()
            .map(|(path, (oid, mode))| (path, oid, mode))
            .collect();
        let tree_id = self.build_tree_from_entries(&entries)?;
        self.create_commit(tree_id, Some(parent), message)
    }

    /// Diff two commits.
    pub fn diff(&self, from: ObjectId, to: ObjectId) -> Result<Vec<DiffEntry>, JailError> {
        let from_tree = self.commit_tree(from)?;
        let to_tree = self.commit_tree(to)?;

        let mut from_map = BTreeMap::new();
        let mut to_map = BTreeMap::new();

        self.flatten_tree(from_tree, "", &mut from_map)?;
        self.flatten_tree(to_tree, "", &mut to_map)?;

        let mut changes = Vec::new();

        for (path, (to_oid, _)) in &to_map {
            match from_map.get(path) {
                None => changes.push(DiffEntry {
                    path: path.clone(),
                    kind: DiffKind::Added,
                }),
                Some((from_oid, _)) if from_oid != to_oid => changes.push(DiffEntry {
                    path: path.clone(),
                    kind: DiffKind::Modified,
                }),
                _ => {}
            }
        }

        for path in from_map.keys() {
            if !to_map.contains_key(path) {
                changes.push(DiffEntry {
                    path: path.clone(),
                    kind: DiffKind::Deleted,
                });
            }
        }

        Ok(changes)
    }

    /// Materialize a commit's changes into a jail upper directory.
    ///
    /// Diffs the commit against the project's HEAD and writes only the
    /// changed/added files. Deleted files are written as whiteout markers
    /// (empty files named `.wh.<filename>`) for overlayfs compatibility,
    /// or simply not written (the overlay layer handles deletions via its
    /// own whiteout set at runtime).
    /// Materialize a commit's changes into a jail upper directory.
    ///
    /// Diffs the commit against the project's HEAD and writes changed/added
    /// files into `upper_dir`. Returns the list of paths that were deleted
    /// relative to HEAD — the caller must pass these to the overlay as
    /// initial whiteouts.
    pub fn materialize_commit(
        &self,
        commit_id: ObjectId,
        upper_dir: &Path,
    ) -> Result<Vec<String>, JailError> {
        let head_id = self.head_commit_id()?;
        if commit_id == head_id {
            return Ok(Vec::new());
        }

        let head_tree = self.commit_tree(head_id)?;
        let commit_tree = self.commit_tree(commit_id)?;

        let mut head_map = BTreeMap::new();
        let mut commit_map = BTreeMap::new();
        self.flatten_tree(head_tree, "", &mut head_map)?;
        self.flatten_tree(commit_tree, "", &mut commit_map)?;

        // Write files that were added or modified relative to HEAD.
        for (path, (oid, _mode)) in &commit_map {
            let changed = match head_map.get(path) {
                None => true,
                Some((head_oid, _)) => head_oid != oid,
            };
            if changed {
                let data = self.find_object(*oid)?;
                let full_path = upper_dir.join(path);
                if let Some(parent) = full_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&full_path, &data)?;
            }
        }

        // Collect paths deleted relative to HEAD.
        let deleted: Vec<String> = head_map
            .keys()
            .filter(|path| !commit_map.contains_key(*path))
            .cloned()
            .collect();

        Ok(deleted)
    }
}
