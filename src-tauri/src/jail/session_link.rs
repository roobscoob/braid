//! Session symlink management.
//!
//! Claude Code stores session data at `~/.claude/projects/<encoded-cwd>/`.
//! When the jail CWD differs from the real project CWD, we create a symlink
//! so `--resume` can find sessions from the original project.

use std::path::{Path, PathBuf};

use crate::jail::error::JailError;

/// Encode a path the way Claude Code does: replace path separators with `-`.
fn encode_path(path: &Path) -> Result<String, JailError> {
    // Try canonicalize, but fall back to the absolute path if it fails
    // (e.g. WinFsp mount points can't be canonicalized).
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| JailError::SessionLink(format!("current_dir: {e}")))?
            .join(path)
    };

    let canonical = std::fs::canonicalize(&abs).unwrap_or(abs);

    let mut s = canonical.to_string_lossy().to_string();

    // Strip the \\?\ prefix that Windows canonicalize adds.
    if s.starts_with(r"\\?\") {
        s = s[4..].to_string();
    }

    // Claude CLI encoding: all separators (`:`, `\`, `/`) become `-`.
    // e.g., `C:\Users\Rose\projects\braid` → `C--Users-Rose-projects-braid`
    // (`:` → `-`, `\` → `-`, so `C:\` becomes `C--`)
    let encoded = s.replace(':', "-").replace('\\', "-").replace('/', "-");

    Ok(encoded)
}

/// Get the Claude projects directory.
fn claude_projects_dir() -> Result<PathBuf, JailError> {
    let home = dirs_next::home_dir()
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
        .or_else(|| std::env::var("HOME").ok().map(PathBuf::from))
        .ok_or_else(|| JailError::SessionLink("cannot determine home directory".into()))?;

    Ok(home.join(".claude").join("projects"))
}

/// Create a session symlink from the jail's encoded path to the real project's
/// encoded path. This lets Claude's `--resume` find sessions when running
/// inside the jail.
pub fn create_session_link(jail_root: &Path, real_cwd: &Path) -> Result<(), JailError> {
    let projects_dir = claude_projects_dir()?;
    let jail_encoded = encode_path(jail_root)?;
    let real_encoded = encode_path(real_cwd)?;

    let jail_link = projects_dir.join(&jail_encoded);
    let real_target = projects_dir.join(&real_encoded);

    // Don't create a self-link.
    if jail_link == real_target {
        return Ok(());
    }

    // Ensure the real target directory exists.
    std::fs::create_dir_all(&real_target)?;

    // Remove any existing link/directory at the jail path.
    if jail_link.exists() || jail_link.symlink_metadata().is_ok() {
        let meta = jail_link
            .symlink_metadata()
            .map_err(|e| JailError::SessionLink(format!("stat: {e}")))?;
        if meta.file_type().is_symlink() || meta.file_type().is_dir() {
            // On Windows, try removing as a symlink first, then as a junction.
            #[cfg(target_os = "windows")]
            {
                let _ = std::fs::remove_dir(&jail_link);
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = std::fs::remove_file(&jail_link);
            }
        }
    }

    // Create the symlink/junction.
    #[cfg(target_os = "windows")]
    {
        // Try directory symlink first (requires developer mode).
        // Fall back to junction which doesn't require elevation.
        let result = std::os::windows::fs::symlink_dir(&real_target, &jail_link);
        if result.is_err() {
            // Use cmd /c mklink /J for junction.
            let status = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&jail_link)
                .arg(&real_target)
                .output()
                .map_err(|e| JailError::SessionLink(format!("mklink /J: {e}")))?;
            if !status.status.success() {
                return Err(JailError::SessionLink(format!(
                    "junction creation failed: {}",
                    String::from_utf8_lossy(&status.stderr)
                )));
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::os::unix::fs::symlink(&real_target, &jail_link)
            .map_err(|e| JailError::SessionLink(format!("symlink: {e}")))?;
    }

    Ok(())
}

/// Remove the session symlink for a jail.
pub fn remove_session_link(jail_root: &Path) -> Result<(), JailError> {
    let projects_dir = claude_projects_dir()?;
    let jail_encoded = encode_path(jail_root)?;
    let jail_link = projects_dir.join(&jail_encoded);

    if jail_link.symlink_metadata().is_ok() {
        #[cfg(target_os = "windows")]
        {
            let _ = std::fs::remove_dir(&jail_link);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = std::fs::remove_file(&jail_link);
        }
    }

    Ok(())
}
