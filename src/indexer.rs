//! The incremental indexing pipeline.
//!
//! Repositories are processed sequentially; within a repository the
//! per-file symbol extraction runs in parallel via rayon while a
//! single writer applies the results. Every file is committed in its
//! own transaction (symbol rows plus file state together), so a crash
//! can never leave the mtime bookkeeping and the symbol rows out of
//! sync.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use rustix::fs::{FlockOperation, flock};

use crate::config::{Config, IgnoreMatcher};
use crate::discover::{self, Repo};
use crate::git;
use crate::lang::{Kind, Lang, Symbol};
use crate::normalize::normalize;

/// Counters describing one indexing run.
#[derive(Debug, Default)]
pub struct Stats {
    /// Repositories seen during discovery.
    pub repos: usize,
    /// Repositories skipped via the HEAD/dirty fast path.
    pub repos_skipped: usize,
    /// Files indexed (new or changed).
    pub files_indexed: usize,
    /// Files removed from the index.
    pub files_removed: usize,
    /// Symbol rows written.
    pub symbols: usize,
    /// Wall-clock duration of the run in milliseconds.
    pub duration_ms: u128,
}

/// The per-file work result produced by the parallel extraction stage.
struct FileResult {
    /// The repo-relative path.
    path: String,
    /// The file mtime in nanoseconds.
    mtime_ns: i64,
    /// The file size in bytes.
    size: i64,
    /// The detected language, when any.
    lang: Option<Lang>,
    /// The extracted code symbols.
    symbols: Vec<Symbol>,
    /// The normalized whole path for the file row.
    file_norm: String,
}

/// The lock guard held for the duration of an indexing run.
///
/// The lock file descriptor is released automatically on drop (and by
/// the kernel on process death), which makes stale locks impossible.
pub struct Lock {
    /// The open lock file handle.
    _file: std::fs::File,
}

/// Try to take the indexer lock beside the database file.
///
/// @param db_path the database file path
/// @return the held lock, or +nil+ when another indexer runs
pub fn try_lock(db_path: &Path) -> Result<Option<Lock>> {
    let path = db_path.with_extension("lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(&path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Some(Lock { _file: file })),
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(None),
        Err(err) => Err(err).context("failed to take the indexer lock"),
    }
}

/// Run one incremental indexing pass.
///
/// @param conn the open database connection
/// @param config the loaded configuration
/// @param full whether to ignore stored state and re-extract all files
/// @return the run statistics
pub fn run(
    conn: &mut Connection,
    config: &Config,
    full: bool,
) -> Result<Stats> {
    let started = Instant::now();
    let mut stats = Stats::default();

    let repos = discover::discover(&config.index.paths, &config.index.nested)?;
    stats.repos = repos.len();

    prune_vanished_repos(conn, &repos)?;

    // Compile the ignore list once for the whole run
    let ignores = config.index.ignore_matcher()?;

    for repo in &repos {
        index_repo(conn, config, repo, full, &ignores, &mut stats)
            .with_context(|| {
                format!("failed to index {}", repo.root.display())
            })?;
    }

    stats.duration_ms = started.elapsed().as_millis();
    Ok(stats)
}

/// Delete database repositories whose root vanished from discovery.
///
/// @param conn the open database connection
/// @param repos the currently discovered repositories
fn prune_vanished_repos(conn: &Connection, repos: &[Repo]) -> Result<()> {
    let known: Vec<(i64, String)> = conn
        .prepare("SELECT id, root FROM repos")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, root) in known {
        let still_there = repos
            .iter()
            .any(|repo| repo.root.as_os_str() == root.as_str());
        if !still_there {
            conn.execute("DELETE FROM repos WHERE id = ?1", [id])?;
        }
    }
    Ok(())
}

/// Index a single repository incrementally.
///
/// @param conn the open database connection
/// @param config the loaded configuration
/// @param repo the repository to index
/// @param full whether to ignore stored state
/// @param ignores the compiled ignore matcher
/// @param stats the run statistics to update
fn index_repo(
    conn: &mut Connection,
    config: &Config,
    repo: &Repo,
    full: bool,
    ignores: &IgnoreMatcher,
    stats: &mut Stats,
) -> Result<()> {
    let head = git::head_commit(&repo.root);
    let dirty = git::is_dirty(&repo.root);

    let stored: Option<(i64, Option<String>, bool)> = conn
        .query_row(
            "SELECT id, head, dirty FROM repos WHERE root = ?1",
            [repo.root.to_string_lossy()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
        )
        .optional()?;

    // Fast path: unchanged HEAD and a clean worktree on both sides
    // means nothing to do — skip even the stat sweep
    if !full
        && let Some((_, stored_head, stored_dirty)) = &stored
        && stored_head == &head
        && !stored_dirty
        && !dirty
    {
        stats.repos_skipped += 1;
        return Ok(());
    }

    let repo_id = match stored {
        Some((id, _, _)) => id,
        None => {
            conn.execute(
                "INSERT INTO repos (name, root, head, dirty, indexed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    repo.name,
                    repo.root.to_string_lossy(),
                    head,
                    dirty as i64,
                    now()
                ],
            )?;
            conn.last_insert_rowid()
        }
    };

    // Build the current tracked-file state (path -> mtime/size),
    // skipping symlinks, gitlinks, ignored paths and vanished files;
    // files dropped here also fall out of the index via the removal
    // diff below
    let mut current: HashMap<String, (i64, i64)> = HashMap::new();
    for file in git::ls_files(&repo.root)? {
        if !git::is_regular_file(file.mode) {
            continue;
        }
        if ignores.matches(&file.path) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(repo.root.join(&file.path))
        else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        current.insert(file.path, (mtime_ns(&meta), meta.len() as i64));
    }

    // Diff against the stored file state
    let stored_files: HashMap<String, (i64, i64)> = conn
        .prepare("SELECT path, mtime_ns, size FROM files WHERE repo_id = ?1")?
        .query_map([repo_id], |row| {
            Ok((row.get(0)?, (row.get(1)?, row.get(2)?)))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let to_index: Vec<(String, i64, i64)> = current
        .iter()
        .filter(|(path, state)| full || stored_files.get(*path) != Some(state))
        .map(|(path, &(mtime, size))| (path.clone(), mtime, size))
        .collect();
    let to_remove: Vec<&String> = stored_files
        .keys()
        .filter(|path| !current.contains_key(*path))
        .collect();

    // Extract new/changed files in parallel; extraction is pure and
    // returns row batches for the single writer below
    let results: Vec<FileResult> = to_index
        .par_iter()
        .map(|(path, mtime, size)| {
            extract_file(&repo.root, path, *mtime, *size, config)
        })
        .collect();

    // Apply results, one transaction per file
    for result in results {
        let txn = conn.transaction()?;
        txn.execute(
            "DELETE FROM files WHERE repo_id = ?1 AND path = ?2",
            params![repo_id, result.path],
        )?;
        txn.execute(
            "INSERT INTO files
               (repo_id, path, path_norm, mtime_ns, size, lang)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                repo_id,
                result.path,
                result.file_norm,
                result.mtime_ns,
                result.size,
                result.lang.map(Lang::as_str)
            ],
        )?;
        let file_id = txn.last_insert_rowid();

        // The file symbol is the whole repo-relative path, so path
        // fragments (`spec/support`) are searchable like any symbol
        txn.execute(
            "INSERT INTO symbols
               (repo_id, file_id, line, kind, name, scope, name_norm,
                qual_norm)
             VALUES (?1, ?2, NULL, ?3, ?4, NULL, ?5, ?5)",
            params![
                repo_id,
                file_id,
                Kind::File.as_str(),
                result.path,
                result.file_norm
            ],
        )?;
        stats.symbols += 1;

        let mut insert = txn.prepare_cached(
            "INSERT INTO symbols
               (repo_id, file_id, line, kind, name, scope, name_norm,
                qual_norm)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for symbol in &result.symbols {
            let name_norm = normalize(&symbol.name);
            // The qualified form makes the enclosing scope matchable
            // (`users_api/api_v1 find` == UsersApi::ApiV1::Find)
            let qual_norm = match &symbol.scope {
                Some(scope) => normalize(&format!("{scope}::{}", symbol.name)),
                None => name_norm.clone(),
            };
            insert.execute(params![
                repo_id,
                file_id,
                symbol.line,
                symbol.kind.as_str(),
                symbol.name,
                symbol.scope,
                name_norm,
                qual_norm
            ])?;
            stats.symbols += 1;
        }
        drop(insert);
        txn.commit()?;
        stats.files_indexed += 1;
    }

    for path in to_remove {
        conn.execute(
            "DELETE FROM files WHERE repo_id = ?1 AND path = ?2",
            params![repo_id, path],
        )?;
        stats.files_removed += 1;
    }

    conn.execute(
        "UPDATE repos SET head = ?1, dirty = ?2, indexed_at = ?3
         WHERE id = ?4",
        params![head, dirty as i64, now(), repo_id],
    )?;
    Ok(())
}

/// Extract one file into its row batch.
///
/// Extraction failures are deliberately swallowed into a bare file row
/// (with a stderr note) — one broken file must never abort the run.
///
/// @param root the repository root
/// @param path the repo-relative file path
/// @param mtime_ns the file mtime in nanoseconds
/// @param size the file size in bytes
/// @param config the loaded configuration
/// @return the per-file result batch
fn extract_file(
    root: &Path,
    path: &str,
    mtime_ns: i64,
    size: i64,
    config: &Config,
) -> FileResult {
    let file_norm = normalize(path);
    let mut result = FileResult {
        path: path.to_string(),
        mtime_ns,
        size,
        lang: None,
        symbols: Vec::new(),
        file_norm,
    };

    if size as u64 > config.index.max_file_size {
        return result;
    }
    let Ok(source) = std::fs::read(root.join(path)) else {
        return result;
    };
    // Binary sniff: a NUL byte within the first 4KiB disqualifies
    if source[..source.len().min(4096)].contains(&0) {
        return result;
    }

    let lang = Lang::detect(path).or_else(|| {
        let basename = path.rsplit('/').next().unwrap_or(path);
        if basename.contains('.') {
            return None;
        }
        let first_line = source.split(|&byte| byte == b'\n').next()?;
        Lang::from_shebang(&String::from_utf8_lossy(first_line))
    });
    let Some(lang) = lang else {
        return result;
    };
    result.lang = Some(lang);

    match lang.extract(&source, config.index.resolve_yaml) {
        Ok(symbols) => result.symbols = symbols,
        Err(err) => {
            eprintln!(
                "kartei: extraction failed for {}: {err:#}",
                root.join(path).display()
            );
        }
    }
    result
}

/// The current unix timestamp in seconds.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// The mtime of a file in nanoseconds since the unix epoch.
///
/// @param meta the file metadata
/// @return the mtime in nanoseconds
fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i64)
        .unwrap_or(0)
}
