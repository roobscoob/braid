//! In-memory version control for jail snapshots.
//!
//! Uses the project's existing `.git` repo as a read-only baseline.
//! All new objects (blobs, trees, commits) are stored in memory.
//! The conversation tree maps directly to an in-memory commit graph.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use gix::objs::tree::Entry as TreeEntry;
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
    /// The jail root (for resolving `.gitignore` files).
    source_root: PathBuf,
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
            mutations: RwLock::new(BTreeMap::new()),
            ignore: RwLock::new(ignore),
        }
    }

    /// Load `.gitignore` patterns from the source root.
    fn load_ignore(root: &Path) -> gix::ignore::Search {
        let mut search = gix::ignore::Search::default();
        let gitignore_path = root.join(".gitignore");
        if gitignore_path.exists() {
            if let Ok(bytes) = std::fs::read(&gitignore_path) {
                search.add_patterns_buffer(&bytes, gitignore_path, Some(root));
            }
        }
        search
    }

    /// Check if a relative path is ignored.
    pub fn is_ignored(&self, rel_path: &str, is_dir: bool) -> bool {
        // Always track .gitignore itself.
        if rel_path == ".gitignore" {
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

        // If .gitignore was created, re-evaluate everything.
        if rel_path == ".gitignore" {
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

        // If .gitignore was modified, re-evaluate everything.
        if rel_path == ".gitignore" {
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

    /// Record a rename (old path deleted, new path created).
    pub fn record_rename(&self, old_rel: &str, new_rel: &str, is_dir: bool) {
        self.record_delete(old_rel);
        self.record_create(new_rel, is_dir);
    }

    /// Reload `.gitignore` and re-check all tracked mutations.
    fn reload_ignore(&self) {
        let new_ignore = Self::load_ignore(&self.source_root);
        *self.ignore.write().unwrap() = new_ignore;

        // Re-evaluate: remove mutations that are now ignored.
        let ignore = self.ignore.read().unwrap();
        let mut mutations = self.mutations.write().unwrap();
        mutations.retain(|path, _| {
            if path == ".gitignore" {
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

    /// Flatten a tree into a map of relative path → blob OID.
    fn flatten_tree(
        &self,
        tree_id: ObjectId,
        prefix: &str,
        out: &mut BTreeMap<String, ObjectId>,
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
                out.insert(path, entry.oid.into());
            }
        }
        Ok(())
    }

    /// Build a nested tree from a flat list of (path, blob_oid) pairs.
    fn build_tree_from_entries(
        &self,
        entries: &[(String, ObjectId)],
    ) -> Result<ObjectId, JailError> {
        let mut files: Vec<TreeEntry> = Vec::new();
        let mut subdirs: BTreeMap<String, Vec<(String, ObjectId)>> = BTreeMap::new();

        for (path, oid) in entries {
            let parts: Vec<&str> = path.splitn(2, '/').collect();
            if parts.len() == 1 {
                files.push(TreeEntry {
                    mode: gix::objs::tree::EntryKind::Blob.into(),
                    filename: parts[0].into(),
                    oid: *oid,
                });
            } else {
                subdirs
                    .entry(parts[0].to_string())
                    .or_default()
                    .push((parts[1].to_string(), *oid));
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
                        file_map.insert(mutation.rel_path.clone(), blob_id);
                    }
                }
                MutationKind::Deleted => {
                    file_map.remove(&mutation.rel_path);
                }
            }
        }

        let entries: Vec<(String, ObjectId)> = file_map.into_iter().collect();
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

        for (path, to_oid) in &to_map {
            match from_map.get(path) {
                None => changes.push(DiffEntry {
                    path: path.clone(),
                    kind: DiffKind::Added,
                }),
                Some(from_oid) if from_oid != to_oid => changes.push(DiffEntry {
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
}
