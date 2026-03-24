//! WinFsp-based overlay filesystem for Windows.
//!
//! Mounts a virtual filesystem that reads from a "lower" (source) directory
//! and redirects all writes to an "upper" (overlay) directory. The source
//! is never modified.

#![cfg(target_os = "windows")]

use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use crate::jail::vcs::MutationTracker;

use winfsp::FspError;
use winfsp_sys::{FILE_ACCESS_RIGHTS, FILE_FLAGS_AND_ATTRIBUTES};
use winfsp::filesystem::{
    DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext, OpenFileInfo, VolumeInfo,
    WideNameInfo,
};
use winfsp::host::{FileSystemHost, VolumeParams};
use winfsp::U16CStr;

use crate::jail::cow::{CowLayer, CowMount};
use crate::jail::error::JailError;

// ─── File context ────────────────────────────────────────────────────────────

/// Represents an open file handle in the overlay.
pub struct OverlayFileContext {
    /// Relative path (forward-slash separated).
    rel_path: String,
    /// Resolved path on the real filesystem (upper or lower).
    real_path: PathBuf,
    /// Open file handle. None for directories.
    file: Option<Mutex<File>>,
    /// Whether this file is in the upper (writable) layer.
    is_upper: bool,
    /// Whether this is a directory.
    is_dir: bool,
}

// ─── Overlay filesystem ──────────────────────────────────────────────────────

/// The overlay filesystem context.
pub struct OverlayFs {
    /// Read-only source directory.
    lower: PathBuf,
    /// Writable overlay directory.
    upper: PathBuf,
    /// Relative paths that have been deleted.
    whiteouts: RwLock<std::collections::HashSet<String>>,
    /// Mutation tracker — records changes for the VCS.
    tracker: Arc<MutationTracker>,
}

impl OverlayFs {
    fn new(lower: PathBuf, upper: PathBuf, tracker: Arc<MutationTracker>) -> Self {
        Self {
            lower,
            upper,
            whiteouts: RwLock::new(std::collections::HashSet::new()),
            tracker,
        }
    }

    /// Convert a WinFsp path (`\foo\bar`) to a relative path string.
    fn to_rel(name: &U16CStr) -> String {
        let s = name.to_string_lossy();
        s.trim_start_matches('\\').replace('\\', "/")
    }

    /// Resolve a relative path. Upper takes priority. None if whited out.
    fn resolve(&self, rel: &str) -> Option<(PathBuf, bool)> {
        if self.whiteouts.read().unwrap().contains(rel) {
            return None;
        }
        // Root directory: return lower (or upper if it exists).
        if rel.is_empty() {
            return Some((self.lower.clone(), false));
        }
        let upper_path = self.upper.join(rel);
        if upper_path.exists() {
            return Some((upper_path, true));
        }
        let lower_path = self.lower.join(rel);
        if lower_path.exists() {
            return Some((lower_path, false));
        }
        None
    }

    /// Copy a file/dir from lower to upper. Returns the upper path.
    fn copy_up(&self, rel: &str) -> std::io::Result<PathBuf> {
        let upper_path = self.upper.join(rel);
        if upper_path.exists() {
            return Ok(upper_path);
        }
        if let Some(parent) = upper_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lower_path = self.lower.join(rel);
        if lower_path.exists() {
            if lower_path.is_dir() {
                fs::create_dir_all(&upper_path)?;
            } else {
                fs::copy(&lower_path, &upper_path)?;
            }
        }
        Ok(upper_path)
    }

    /// Fill a FileInfo from filesystem metadata.
    fn fill_info(path: &Path, info: &mut FileInfo) -> std::io::Result<()> {
        let meta = fs::metadata(path)?;
        info.file_size = meta.len();
        info.allocation_size = (meta.len() + 4095) & !4095;
        info.file_attributes = if meta.is_dir() { 0x10 } else { meta.file_attributes() };
        if info.file_attributes == 0 {
            info.file_attributes = 0x80; // FILE_ATTRIBUTE_NORMAL
        }
        info.creation_time = meta.creation_time();
        info.last_access_time = meta.last_access_time();
        info.last_write_time = meta.last_write_time();
        info.change_time = meta.last_write_time();
        Ok(())
    }

    /// List directory entries by merging lower + upper.
    fn list_dir(&self, dir_rel: &str) -> Vec<(String, PathBuf)> {
        let mut entries = std::collections::BTreeMap::<String, PathBuf>::new();
        let whiteouts = self.whiteouts.read().unwrap();

        // Lower first.
        let lower_dir = if dir_rel.is_empty() {
            self.lower.clone()
        } else {
            self.lower.join(dir_rel)
        };
        if let Ok(rd) = fs::read_dir(&lower_dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let child_rel = if dir_rel.is_empty() {
                    name.clone()
                } else {
                    format!("{dir_rel}/{name}")
                };
                if !whiteouts.contains(&child_rel) {
                    entries.insert(name, e.path());
                }
            }
        }

        // Upper overrides.
        let upper_dir = if dir_rel.is_empty() {
            self.upper.clone()
        } else {
            self.upper.join(dir_rel)
        };
        if let Ok(rd) = fs::read_dir(&upper_dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                entries.insert(name, e.path());
            }
        }

        entries.into_iter().collect()
    }
}

// ─── FileSystemContext ───────────────────────────────────────────────────────

impl FileSystemContext for OverlayFs {
    type FileContext = OverlayFileContext;

    fn get_security_by_name(
        &self,
        file_name: &U16CStr,
        _security_descriptor: Option<&mut [c_void]>,
        _resolve_reparse_points: impl FnOnce(&U16CStr) -> Option<FileSecurity>,
    ) -> winfsp::Result<FileSecurity> {
        let rel = Self::to_rel(file_name);
        let (path, _) = self.resolve(&rel).ok_or(FspError::IO(ErrorKind::NotFound))?;
        let meta = fs::metadata(&path)?;
        Ok(FileSecurity {
            attributes: if meta.is_dir() { 0x10 } else { meta.file_attributes() },
            reparse: false,
            sz_security_descriptor: 0,
        })
    }

    fn open(
        &self,
        file_name: &U16CStr,
        _create_options: u32,
        _granted_access: FILE_ACCESS_RIGHTS,
        file_info: &mut OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {
        let rel = Self::to_rel(file_name);
        let (path, is_upper) = self.resolve(&rel).ok_or(FspError::IO(ErrorKind::NotFound))?;
        let meta = fs::metadata(&path)?;
        let is_dir = meta.is_dir();

        Self::fill_info(&path, file_info.as_mut())?;

        let file = if !is_dir {
            let f = OpenOptions::new()
                .read(true)
                .write(is_upper)
                .open(&path)?;
            Some(Mutex::new(f))
        } else {
            None
        };

        Ok(OverlayFileContext {
            rel_path: rel,
            real_path: path,
            file,
            is_upper,
            is_dir,
        })
    }

    fn close(&self, _context: Self::FileContext) {}

    fn read(
        &self,
        context: &Self::FileContext,
        buffer: &mut [u8],
        offset: u64,
    ) -> winfsp::Result<u32> {
        let guard = context.file.as_ref().ok_or(FspError::IO(ErrorKind::Other))?;
        let mut file = guard.lock().unwrap();
        file.seek(SeekFrom::Start(offset))?;
        let n = file.read(buffer)?;
        Ok(n as u32)
    }

    fn write(
        &self,
        context: &Self::FileContext,
        buffer: &[u8],
        offset: u64,
        write_to_eof: bool,
        _constrained_io: bool,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<u32> {
        if !context.is_upper {
            return Err(FspError::IO(ErrorKind::PermissionDenied));
        }
        let guard = context.file.as_ref().ok_or(FspError::IO(ErrorKind::Other))?;
        let mut file = guard.lock().unwrap();
        if write_to_eof {
            file.seek(SeekFrom::End(0))?;
        } else {
            file.seek(SeekFrom::Start(offset))?;
        }
        let n = file.write(buffer)?;
        drop(file);
        Self::fill_info(&context.real_path, file_info)?;
        self.tracker.record_modify(&context.rel_path);
        Ok(n as u32)
    }

    fn create(
        &self,
        file_name: &U16CStr,
        create_options: u32,
        _granted_access: FILE_ACCESS_RIGHTS,
        _file_attributes: FILE_FLAGS_AND_ATTRIBUTES,
        _security_descriptor: Option<&[c_void]>,
        _allocation_size: u64,
        _extra_buffer: Option<&[u8]>,
        _extra_buffer_is_reparse_point: bool,
        file_info: &mut OpenFileInfo,
    ) -> winfsp::Result<Self::FileContext> {
        let rel = Self::to_rel(file_name);
        self.whiteouts.write().unwrap().remove(&rel);

        let upper_path = self.copy_up(&rel)?;
        let is_dir = create_options & 0x1 != 0; // FILE_DIRECTORY_FILE

        if is_dir {
            fs::create_dir_all(&upper_path)?;
        } else if !upper_path.exists() {
            if let Some(parent) = upper_path.parent() {
                fs::create_dir_all(parent)?;
            }
            File::create(&upper_path)?;
        }

        Self::fill_info(&upper_path, file_info.as_mut())?;

        let file = if !is_dir {
            let f = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&upper_path)?;
            Some(Mutex::new(f))
        } else {
            None
        };

        self.tracker.record_create(&rel, is_dir);

        Ok(OverlayFileContext {
            rel_path: rel,
            real_path: upper_path,
            file,
            is_upper: true,
            is_dir,
        })
    }

    fn overwrite(
        &self,
        context: &Self::FileContext,
        _file_attributes: FILE_FLAGS_AND_ATTRIBUTES,
        _replace_file_attributes: bool,
        _allocation_size: u64,
        _extra_buffer: Option<&[u8]>,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        if !context.is_upper {
            return Err(FspError::IO(ErrorKind::PermissionDenied));
        }
        if let Some(ref guard) = context.file {
            let file = guard.lock().unwrap();
            file.set_len(0)?;
        }
        Self::fill_info(&context.real_path, file_info)?;
        self.tracker.record_modify(&context.rel_path);
        Ok(())
    }

    fn get_file_info(
        &self,
        context: &Self::FileContext,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        Self::fill_info(&context.real_path, file_info)?;
        Ok(())
    }

    fn read_directory(
        &self,
        context: &Self::FileContext,
        _pattern: Option<&U16CStr>,
        marker: DirMarker,
        buffer: &mut [u8],
    ) -> winfsp::Result<u32> {
        if !context.is_dir {
            return Err(FspError::IO(ErrorKind::NotADirectory));
        }

        // Determine relative path of this directory.
        let dir_rel = if context.real_path == self.lower || context.real_path == self.upper {
            String::new()
        } else if let Ok(rel) = context.real_path.strip_prefix(&self.upper) {
            rel.to_string_lossy().replace('\\', "/")
        } else if let Ok(rel) = context.real_path.strip_prefix(&self.lower) {
            rel.to_string_lossy().replace('\\', "/")
        } else {
            String::new()
        };

        let entries = self.list_dir(&dir_rel);

        let mut cursor = 0u32;
        let mut past_marker = marker.is_none();

        for (name, path) in &entries {
            if !past_marker {
                if let Some(marker_cstr) = marker.inner_as_cstr() {
                    if name.as_str() == AsRef::<str>::as_ref(&marker_cstr.to_string_lossy()) {
                        past_marker = true;
                    }
                }
                continue;
            }

            let mut entry: DirInfo = DirInfo::new();
            Self::fill_info(path, entry.file_info_mut())?;
            entry
                .set_name(std::ffi::OsStr::new(name))
                .map_err(|_| FspError::IO(ErrorKind::InvalidInput))?;
            if !entry.append_to_buffer(buffer, &mut cursor) {
                break;
            }
        }

        DirInfo::<255>::finalize_buffer(buffer, &mut cursor);
        Ok(cursor)
    }

    fn get_volume_info(&self, out: &mut VolumeInfo) -> winfsp::Result<()> {
        out.total_size = 100 * 1024 * 1024 * 1024; // 100 GB
        out.free_size = 50 * 1024 * 1024 * 1024; // 50 GB
        Ok(())
    }

    fn cleanup(&self, context: &Self::FileContext, _file_name: Option<&U16CStr>, flags: u32) {
        if flags & 1 != 0 {
            // FspCleanupDelete
            if context.is_dir {
                let _ = fs::remove_dir_all(&context.real_path);
            } else {
                let _ = fs::remove_file(&context.real_path);
            }
            if !context.is_upper {
                if let Ok(rel) = context.real_path.strip_prefix(&self.lower) {
                    self.whiteouts
                        .write()
                        .unwrap()
                        .insert(rel.to_string_lossy().replace('\\', "/"));
                }
            }
            self.tracker.record_delete(&context.rel_path);
        }
    }

    fn set_delete(
        &self,
        _context: &Self::FileContext,
        _file_name: &U16CStr,
        _delete_file: bool,
    ) -> winfsp::Result<()> {
        Ok(())
    }

    fn rename(
        &self,
        context: &Self::FileContext,
        _file_name: &U16CStr,
        new_file_name: &U16CStr,
        _replace_if_exists: bool,
    ) -> winfsp::Result<()> {
        let new_rel = Self::to_rel(new_file_name);
        let new_upper = self.upper.join(&new_rel);

        if let Some(parent) = new_upper.parent() {
            fs::create_dir_all(parent)?;
        }

        if context.is_upper {
            fs::rename(&context.real_path, &new_upper)?;
        } else {
            if context.is_dir {
                copy_dir_all(&context.real_path, &new_upper)?;
            } else {
                fs::copy(&context.real_path, &new_upper)?;
            }
            if let Ok(rel) = context.real_path.strip_prefix(&self.lower) {
                self.whiteouts
                    .write()
                    .unwrap()
                    .insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }

        self.whiteouts.write().unwrap().remove(&new_rel);
        self.tracker.record_rename(&context.rel_path, &new_rel, context.is_dir);
        Ok(())
    }

    fn flush(
        &self,
        context: Option<&Self::FileContext>,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        if let Some(ctx) = context {
            if let Some(ref guard) = ctx.file {
                guard.lock().unwrap().sync_all()?;
            }
            Self::fill_info(&ctx.real_path, file_info)?;
        }
        Ok(())
    }

    fn set_file_size(
        &self,
        context: &Self::FileContext,
        new_size: u64,
        set_allocation_size: bool,
        file_info: &mut FileInfo,
    ) -> winfsp::Result<()> {
        if !context.is_upper {
            return Err(FspError::IO(ErrorKind::PermissionDenied));
        }
        if !set_allocation_size {
            if let Some(ref guard) = context.file {
                guard.lock().unwrap().set_len(new_size)?;
            }
        }
        Self::fill_info(&context.real_path, file_info)?;
        Ok(())
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let dest = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

// ─── CowLayer implementation ────────────────────────────────────────────────

/// WinFsp overlay CoW backend.
pub struct WinFspCow;

impl WinFspCow {
    pub fn new() -> Self {
        Self
    }
}

impl CowLayer for WinFspCow {
    fn name(&self) -> &'static str {
        "winfsp-overlay"
    }

    fn create(&self, source: &Path, dest: &Path, tracker: Arc<MutationTracker>) -> Result<CowMount, JailError> {
        // Ensure WinFsp DLL is discoverable at runtime. The installer puts it
        // in "C:\Program Files (x86)\WinFsp\bin" which isn't on PATH by default.
        if let Ok(winfsp_dir) = std::env::var("WINFSP_INSTALL_DIR") {
            let bin = PathBuf::from(&winfsp_dir).join("bin");
            if bin.exists() {
                let path = std::env::var("PATH").unwrap_or_default();
                std::env::set_var("PATH", format!("{};{path}", bin.display()));
            }
        } else {
            let default = Path::new(r"C:\Program Files (x86)\WinFsp\bin");
            if default.exists() {
                let path = std::env::var("PATH").unwrap_or_default();
                if !path.contains(&default.display().to_string()) {
                    std::env::set_var("PATH", format!("{};{path}", default.display()));
                }
            }
        }

        let _init = winfsp::winfsp_init()
            .map_err(|e| JailError::CowSetup(format!("WinFsp init failed: {e:?}")))?;

        let upper = dest.join("upper");
        fs::create_dir_all(&upper)?;

        // WinFsp directory mount points must NOT exist beforehand —
        // WinFsp creates them itself via reparse points.
        let mount_point = dest.join("mount");
        if mount_point.exists() {
            let _ = fs::remove_dir(&mount_point);
        }

        let overlay = OverlayFs::new(
            fs::canonicalize(source)?,
            fs::canonicalize(&upper)?,
            tracker,
        );

        let mut volume_params = VolumeParams::new();
        volume_params
            .filesystem_name("braid-overlay")
            .file_info_timeout(0);

        let mut host = FileSystemHost::new(volume_params, overlay)
            .map_err(|e| JailError::CowSetup(format!("WinFsp host: {e:?}")))?;

        // WinFsp needs an absolute path for directory mount points.
        let abs_mount = if mount_point.is_absolute() {
            mount_point.clone()
        } else {
            std::env::current_dir()?.join(&mount_point)
        };
        host.mount(abs_mount.to_string_lossy().as_ref())
            .map_err(|e| JailError::CowSetup(format!("WinFsp mount: {e:?}")))?;
        host.start()
            .map_err(|e| JailError::CowSetup(format!("WinFsp start: {e:?}")))?;

        // Move the host into the teardown closure so it stays alive
        // and can be properly shut down when the jail is torn down.
        let dest_owned = dest.to_path_buf();
        Ok(CowMount::new(mount_point, dest_owned, move || {
            // Dropping the host stops the WinFsp service and unmounts.
            drop(host);
        }))
    }
}
