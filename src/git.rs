//! Git subprocess wrappers.
//!
//! We deliberately shell out to git instead of linking a git library:
//! `git ls-files` is exactly correct w.r.t. tracked-vs-ignored
//! semantics, trivially debuggable, and costs one cheap subprocess per
//! repository per run.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// A tracked file entry as listed by git.
#[derive(Debug)]
pub struct TrackedFile {
    /// The repo-relative path (forward slashes).
    pub path: String,
    /// The git object mode (eg. 100644, 120000, 160000).
    pub mode: u32,
}

/// List the tracked files of a repository.
///
/// Gitlink entries (submodules, mode 160000) and symlinks (mode
/// 120000) are included in the listing and filtered by the caller.
///
/// @param root the repository root
/// @return the tracked files with their object modes
pub fn ls_files(root: &Path) -> Result<Vec<TrackedFile>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--format=%(objectmode) %(path)"])
        .output()
        .context("failed to run git ls-files")?;
    if !output.status.success() {
        bail!(
            "git ls-files failed in {}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();
    for entry in raw.split('\0') {
        let Some((mode, path)) = entry.split_once(' ') else {
            continue;
        };
        let mode = u32::from_str_radix(mode, 8).unwrap_or(0);
        files.push(TrackedFile {
            path: path.to_string(),
            mode,
        });
    }
    Ok(files)
}

/// Fetch the current HEAD commit of a repository.
///
/// @param root the repository root
/// @return the commit hash, or +nil+ for a repository without commits
pub fn head_commit(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether the repository worktree has uncommitted changes.
///
/// A failing status command counts as dirty so the repository is
/// re-scanned rather than wrongly skipped.
///
/// @param root the repository root
/// @return whether the worktree is dirty
pub fn is_dirty(root: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output();
    match output {
        Ok(output) if output.status.success() => !output.stdout.is_empty(),
        _ => true,
    }
}

/// Whether the given object mode denotes a regular file.
///
/// @param mode the git object mode
/// @return whether the entry is a regular (non-symlink, non-gitlink)
///   file
pub fn is_regular_file(mode: u32) -> bool {
    // Regular files are 100644/100755; 120000 is a symlink and 160000
    // a gitlink (submodule)
    mode & 0o170000 == 0o100000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_file_modes_are_recognized() {
        assert!(is_regular_file(0o100644));
    }

    #[test]
    fn executable_file_modes_are_recognized() {
        assert!(is_regular_file(0o100755));
    }

    #[test]
    fn symlink_modes_are_rejected() {
        assert!(!is_regular_file(0o120000));
    }

    #[test]
    fn gitlink_modes_are_rejected() {
        assert!(!is_regular_file(0o160000));
    }
}
