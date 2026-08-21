//! Git repository discovery.
//!
//! Walks the configured start paths and collects git repository roots.
//! The walk is pruned at every repository root: nested repositories
//! (eg. throwaway clones under a repo's tmp/ directory) never become
//! index candidates — their files are not tracked by the enclosing
//! repository and they are no repositories of interest themselves.
//! The configured nested directories are the exception: below them
//! the walk continues through every repository root, so repositories
//! within repositories (at any depth) are indexed as repositories of
//! their own — named after the enclosing repository plus their path
//! within it (`kartei/kartei`, `chat-unread/tmp/checkout`), so a
//! nested checkout never shares its name with its parent.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// A discovered repository.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Repo {
    /// The repository name: the basename of the root, prefixed by the
    /// enclosing repository's name and path for nested repositories.
    pub name: String,
    /// The absolute root path.
    pub root: PathBuf,
}

/// Discover all git repository roots under the given start paths.
///
/// @param paths the configured start paths
/// @param nested the directories below which nested repositories are
///   discovered in depth
/// @return the discovered repositories, sorted by name
pub fn discover(paths: &[PathBuf], nested: &[PathBuf]) -> Result<Vec<Repo>> {
    let mut repos = Vec::new();
    for path in paths {
        walk(path, nested, None, &mut repos);
    }
    repos.sort();
    repos.dedup();
    Ok(repos)
}

/// Recursively walk a directory, collecting repository roots.
///
/// Unreadable directories are skipped silently — start paths may
/// legitimately contain protected areas and the indexer must not fail
/// on them.
///
/// @param dir the directory to inspect
/// @param nested the directories below which nested repositories are
///   discovered in depth
/// @param parent the enclosing repository, or +nil+ outside of one
/// @param repos the collected repositories so far
fn walk(
    dir: &Path,
    nested: &[PathBuf],
    parent: Option<&Repo>,
    repos: &mut Vec<Repo>,
) {
    // A .git entry (dir or worktree/submodule file) marks a repo root;
    // prune the walk here so nested repositories stay invisible —
    // unless the root lies within a configured nested directory, where
    // the walk continues and every repository below is collected too
    let mut parent = parent;
    let repo;
    if dir.join(".git").exists() {
        repo = Repo {
            name: repo_name(dir, parent),
            root: dir.to_path_buf(),
        };
        repos.push(repo.clone());
        if !within_nested(dir, nested) {
            return;
        }
        parent = Some(&repo);
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Skip hidden directories and never follow directory symlinks
        // to avoid walking out of the start path or looping
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        let is_dir =
            entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        if is_dir && !hidden {
            walk(&path, nested, parent, repos);
        }
    }
}

/// Name a repository root.
///
/// A top-level repository is named by its basename. A nested one is
/// named after its enclosing repository plus its path within it
/// (`kartei/kartei`, `chat-unread/tmp/checkout`) — the basename alone
/// would collide with the parent in the classic checkout-in-checkout
/// layout and send every target to the wrong root.
///
/// @param dir the repository root
/// @param parent the enclosing repository, or +nil+ for top-level ones
/// @return the repository name
fn repo_name(dir: &Path, parent: Option<&Repo>) -> String {
    if let Some(parent) = parent
        && let Ok(relative) = dir.strip_prefix(&parent.root)
    {
        let within = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        return format!("{}/{within}", parent.name);
    }
    dir.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string())
}

/// Check whether a directory equals or lies below one of the nested
/// directories.
///
/// Both sides are canonicalized so symlinked or relatively spelled
/// configuration entries still match the walked paths.
///
/// @param dir the directory to check
/// @param nested the configured nested directories
/// @return whether the directory is within a nested directory
fn within_nested(dir: &Path, nested: &[PathBuf]) -> bool {
    let Ok(dir) = dir.canonicalize() else {
        return false;
    };
    nested
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| dir.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a directory including a .git marker directory.
    fn fake_repo(base: &Path, rel: &str) {
        std::fs::create_dir_all(base.join(rel).join(".git")).unwrap();
    }

    #[test]
    fn finds_repos_recursively() {
        let dir = tempfile::tempdir().unwrap();
        fake_repo(dir.path(), "group/alpha");
        fake_repo(dir.path(), "beta");
        let repos = discover(&[dir.path().to_path_buf()], &[]).unwrap();
        assert_eq!(
            repos
                .iter()
                .map(|repo| repo.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
    }

    #[test]
    fn prunes_nested_repos() {
        let dir = tempfile::tempdir().unwrap();
        fake_repo(dir.path(), "outer");
        fake_repo(dir.path(), "outer/tmp/inner");
        let repos = discover(&[dir.path().to_path_buf()], &[]).unwrap();
        assert_eq!(repos.len(), 1);
    }

    #[test]
    fn walks_into_nested_repos_below_configured_roots() {
        let dir = tempfile::tempdir().unwrap();
        fake_repo(dir.path(), "outer");
        fake_repo(dir.path(), "outer/tmp/inner");
        fake_repo(dir.path(), "outer/tmp/inner/deps/deep");
        fake_repo(dir.path(), "other");
        fake_repo(dir.path(), "other/tmp/pruned");
        let nested = [dir.path().join("outer")];
        let repos = discover(&[dir.path().to_path_buf()], &nested).unwrap();
        assert_eq!(
            repos
                .iter()
                .map(|repo| repo.name.as_str())
                .collect::<Vec<_>>(),
            [
                "other",
                "outer",
                "outer/tmp/inner",
                "outer/tmp/inner/deps/deep"
            ]
        );
    }

    #[test]
    fn names_same_named_nested_repos_by_their_parent() {
        let dir = tempfile::tempdir().unwrap();
        fake_repo(dir.path(), "kartei");
        fake_repo(dir.path(), "kartei/kartei");
        let nested = [dir.path().join("kartei")];
        let repos = discover(&[dir.path().to_path_buf()], &nested).unwrap();
        assert_eq!(
            repos
                .iter()
                .map(|repo| repo.name.as_str())
                .collect::<Vec<_>>(),
            ["kartei", "kartei/kartei"]
        );
    }

    #[test]
    fn walks_everything_in_depth_below_a_nested_start_path() {
        let dir = tempfile::tempdir().unwrap();
        fake_repo(dir.path(), "outer");
        fake_repo(dir.path(), "outer/tmp/inner");
        let nested = [dir.path().to_path_buf()];
        let repos = discover(&[dir.path().to_path_buf()], &nested).unwrap();
        assert_eq!(repos.len(), 2);
    }

    #[test]
    fn skips_hidden_directories() {
        let dir = tempfile::tempdir().unwrap();
        fake_repo(dir.path(), ".cache/hidden");
        let repos = discover(&[dir.path().to_path_buf()], &[]).unwrap();
        assert!(repos.is_empty());
    }

    #[test]
    fn accepts_a_start_path_that_is_a_repo_root() {
        let dir = tempfile::tempdir().unwrap();
        fake_repo(dir.path(), ".");
        let repos = discover(&[dir.path().to_path_buf()], &[]).unwrap();
        assert_eq!(repos.len(), 1);
    }
}
