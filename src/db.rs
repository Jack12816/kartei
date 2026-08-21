//! Database bootstrap and connection handling.
//!
//! One SQLite file in WAL mode holds both the incremental file state
//! and the symbol rows, so a per-file re-index is a single atomic
//! transaction and readers are never blocked by the indexing writer.
//! The database is a disposable cache — the repositories are the
//! source of truth — so a schema-version mismatch simply drops and
//! recreates all tables.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

/// The current schema version; bump on any schema change — and on
/// any change to the indexed data itself (eg. the name_norm format),
/// since a version mismatch rebuilds the whole database.
const SCHEMA_VERSION: &str = "8";

/// The schema definition.
const SCHEMA: &str = "
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
) STRICT;

CREATE TABLE repos (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  root TEXT NOT NULL UNIQUE,
  head TEXT,
  dirty INTEGER NOT NULL,
  indexed_at INTEGER NOT NULL
) STRICT;

CREATE TABLE files (
  id INTEGER PRIMARY KEY,
  repo_id INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  path_norm TEXT NOT NULL,
  mtime_ns INTEGER NOT NULL,
  size INTEGER NOT NULL,
  lang TEXT,
  UNIQUE (repo_id, path)
) STRICT;

CREATE TABLE symbols (
  id INTEGER PRIMARY KEY,
  repo_id INTEGER NOT NULL,
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  line INTEGER,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  scope TEXT,
  name_norm TEXT NOT NULL,
  qual_norm TEXT NOT NULL
) STRICT;

CREATE INDEX idx_symbols_repo ON symbols (repo_id);
CREATE INDEX idx_symbols_norm ON symbols (name_norm);
CREATE INDEX idx_symbols_file ON symbols (file_id);
";

/// Open the database, creating directories and schema as needed.
///
/// @param path the database file path
/// @return the ready-to-use connection
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create {}", parent.display())
        })?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;

    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;

    migrate(&conn)?;
    Ok(conn)
}

/// Ensure the schema exists in the current version.
///
/// On a version mismatch all tables are dropped and recreated; the
/// next `kartei index` run repopulates the cache from scratch.
///
/// @param conn the open database connection
fn migrate(conn: &Connection) -> Result<()> {
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .ok();

    if version.as_deref() == Some(SCHEMA_VERSION) {
        return Ok(());
    }

    conn.execute_batch(
        "DROP TABLE IF EXISTS symbols;
         DROP TABLE IF EXISTS files;
         DROP TABLE IF EXISTS repos;
         DROP TABLE IF EXISTS meta;",
    )?;
    conn.execute_batch(SCHEMA)?;
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
        [SCHEMA_VERSION],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open a fresh in-memory-backed temp database for a test.
    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("index.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn bootstraps_schema() {
        let (_dir, conn) = temp_db();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'
                 AND name IN ('meta', 'repos', 'files', 'symbols')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 4);
    }

    #[test]
    fn keeps_data_on_matching_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let conn = open(&path).unwrap();
        conn.execute(
            "INSERT INTO repos (name, root, dirty, indexed_at)
             VALUES ('a', '/a', 0, 0)",
            [],
        )
        .unwrap();
        drop(conn);
        let conn = open(&path).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM repos", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn rebuilds_on_version_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let conn = open(&path).unwrap();
        conn.execute(
            "INSERT INTO repos (name, root, dirty, indexed_at)
             VALUES ('a', '/a', 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE meta SET value = '0' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        drop(conn);
        let conn = open(&path).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM repos", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
