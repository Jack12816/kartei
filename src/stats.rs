//! Index statistics: the stats subcommand output.
//!
//! Without a repository argument it renders one aligned table row per
//! repository (`repo | files | symbols`) plus a totals row; with one it
//! renders a multi-line detail block per matching repository, including
//! a symbol-kind breakdown. The debugging listing dumps the raw index
//! contents of a repository instead: one block per file with its
//! symbols, optionally restricted to a set of symbol kinds.

use std::io::Write;

use anyhow::Result;
use rusqlite::Connection;

/// One per-repository statistics row.
struct RepoStats {
    /// The repository id.
    id: i64,
    /// The repository name.
    name: String,
    /// The absolute repository root.
    root: String,
    /// The indexed HEAD commit, or +nil+ for a repo without commits.
    head: Option<String>,
    /// Whether the worktree was dirty at index time.
    dirty: bool,
    /// The number of indexed files.
    files: i64,
    /// The number of extracted symbols.
    symbols: i64,
}

/// Load the statistics rows, optionally filtered by repository ids.
///
/// @param conn the open database connection
/// @param repo_ids the repository ids to restrict to, or +nil+ for all
/// @return the per-repository rows in name order
fn load(conn: &Connection, repo_ids: Option<&[i64]>) -> Result<Vec<RepoStats>> {
    // Count per repo via correlated subqueries; joining files and
    // symbols at once would build a per-repo cross product
    let mut sql = String::from(
        "SELECT repos.id, repos.name, repos.root, repos.head, repos.dirty,
                (SELECT count(*) FROM files
                  WHERE files.repo_id = repos.id),
                (SELECT count(*) FROM symbols
                  WHERE symbols.repo_id = repos.id)
         FROM repos",
    );
    if let Some(ids) = repo_ids {
        let marks = vec!["?"; ids.len()].join(", ");
        sql.push_str(&format!(" WHERE repos.id IN ({marks})"));
    }
    sql.push_str(" ORDER BY repos.name");

    let rows = conn
        .prepare(&sql)?
        .query_map(
            rusqlite::params_from_iter(repo_ids.unwrap_or_default()),
            |row| {
                Ok(RepoStats {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    root: row.get(2)?,
                    head: row.get(3)?,
                    dirty: row.get::<_, i64>(4)? != 0,
                    files: row.get(5)?,
                    symbols: row.get(6)?,
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Render the global statistics table for all repositories.
///
/// @param conn the open database connection
/// @return the rendered table
pub fn global(conn: &Connection) -> Result<String> {
    let rows = load(conn, None)?;
    let total = RepoStats {
        id: 0,
        name: format!("total ({} repos)", rows.len()),
        root: String::new(),
        head: None,
        dirty: false,
        files: rows.iter().map(|row| row.files).sum(),
        symbols: rows.iter().map(|row| row.symbols).sum(),
    };

    // Size each column to its widest cell (headers and totals included)
    let cells = |row: &RepoStats| {
        (
            row.name.clone(),
            row.files.to_string(),
            row.symbols.to_string(),
        )
    };
    let mut widths = ("repo".len(), "files".len(), "symbols".len());
    for row in rows.iter().chain([&total]) {
        let (name, files, symbols) = cells(row);
        widths.0 = widths.0.max(name.len());
        widths.1 = widths.1.max(files.len());
        widths.2 = widths.2.max(symbols.len());
    }

    let line = |name: &str, files: &str, symbols: &str| {
        format!(
            "{name:<0$} | {files:>1$} | {symbols:>2$}\n",
            widths.0, widths.1, widths.2
        )
    };
    let separator = format!(
        "{}-+-{}-+-{}\n",
        "-".repeat(widths.0),
        "-".repeat(widths.1),
        "-".repeat(widths.2)
    );

    let header = line("repo", "files", "symbols") + &separator;
    let mut out = header.clone();
    for row in &rows {
        let (name, files, symbols) = cells(row);
        out.push_str(&line(&name, &files, &symbols));
    }

    // Repeat the header before the totals so it stays readable right
    // above the shell prompt on long listings
    out.push('\n');
    out.push_str(&header);
    let (name, files, symbols) = cells(&total);
    out.push_str(&line(&name, &files, &symbols));
    Ok(out)
}

/// Render the detail blocks for the given repositories.
///
/// @param conn the open database connection
/// @param repo_ids the repository ids to render
/// @return the rendered blocks separated by blank lines
pub fn detail(conn: &Connection, repo_ids: &[i64]) -> Result<String> {
    let mut out = String::new();
    for (index, row) in load(conn, Some(repo_ids))?.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let head = match (&row.head, row.dirty) {
            (Some(head), true) => format!("{head} (dirty)"),
            (Some(head), false) => head.clone(),
            (None, _) => "none".to_string(),
        };
        out.push_str(&format!(
            "Repo name: {}\nRepo path: {}\nHead commit: {head}\n\
             Files: {}\nSymbols: {}\nSymbol kinds:\n{}",
            row.name,
            row.root,
            row.files,
            row.symbols,
            kind_breakdown(conn, row.id)?
        ));
    }
    Ok(out)
}

/// List the indexed files of the given repositories per file: the
/// path followed by its two-space indented `kind: name` symbol lines.
///
/// With a kind filter only matching symbols are rendered and files
/// without any match are omitted.
///
/// @param conn the open database connection
/// @param repo_ids the repository ids to list
/// @param kinds the symbol kinds to restrict to, empty for all
/// @return the rendered per-file blocks
/// @raise when the kind filter yields no symbols at all
pub fn files(
    conn: &Connection,
    repo_ids: &[i64],
    kinds: &[String],
) -> Result<String> {
    let marks = vec!["?"; repo_ids.len()].join(", ");
    let mut sql = format!(
        "SELECT files.path, symbols.kind, symbols.name
         FROM symbols
         JOIN files ON files.id = symbols.file_id
         JOIN repos ON repos.id = symbols.repo_id
         WHERE symbols.repo_id IN ({marks})"
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = repo_ids
        .iter()
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    if !kinds.is_empty() {
        let kind_marks = vec!["?"; kinds.len()].join(", ");
        sql.push_str(&format!(" AND symbols.kind IN ({kind_marks})"));
        for kind in kinds {
            params.push(Box::new(kind.clone()));
        }
    }
    sql.push_str(" ORDER BY repos.name, files.path, symbols.line");

    // Group the flat symbol rows into per-file blocks; the rows come
    // pre-sorted by path, so a change of path starts the next block
    let mut out = String::new();
    let mut previous: Option<String> = None;
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    for row in rows {
        let (path, kind, name) = row?;
        if previous.as_deref() != Some(&path) {
            out.push_str(&path);
            out.push('\n');
            previous = Some(path);
        }
        out.push_str(&format!("  {kind}: {name}\n"));
    }

    // An empty kind filter is most likely a typo, so answer it with
    // the kinds these repositories actually hold
    if out.is_empty() && !kinds.is_empty() {
        let known = conn
            .prepare(&format!(
                "SELECT DISTINCT kind FROM symbols
                  WHERE repo_id IN ({marks}) ORDER BY kind"
            ))?
            .query_map(rusqlite::params_from_iter(repo_ids), |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        anyhow::bail!(
            "no symbols of kind '{}' (kinds: {})",
            kinds.join(", "),
            known.join(", ")
        );
    }
    Ok(out)
}

/// Summarize the symbol kinds of one repository.
///
/// @param conn the open database connection
/// @param repo_id the repository id
/// @return one two-space indented `kind count` line per kind,
///   sorted by count
fn kind_breakdown(conn: &Connection, repo_id: i64) -> Result<String> {
    let kinds = conn
        .prepare(
            "SELECT kind, count(*) FROM symbols WHERE repo_id = ?1
             GROUP BY kind ORDER BY count(*) DESC, kind",
        )?
        .query_map([repo_id], |row| {
            Ok(format!(
                "  {} {}\n",
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(kinds.join(""))
}

/// Write a rendered statistics text to stdout.
///
/// @param text the rendered output
pub fn print(text: &str) {
    // A broken pipe (eg. `| head`) is a normal way to stop
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open an in-memory database with a small indexed fixture.
    fn conn_with_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE repos (id INTEGER PRIMARY KEY, name TEXT,
               root TEXT, head TEXT, dirty INTEGER, indexed_at INTEGER);
             CREATE TABLE files (id INTEGER PRIMARY KEY,
               repo_id INTEGER, path TEXT, lang TEXT);
             CREATE TABLE symbols (id INTEGER PRIMARY KEY,
               repo_id INTEGER, file_id INTEGER, line INTEGER,
               kind TEXT, name TEXT, scope TEXT, name_norm TEXT);
             INSERT INTO repos VALUES
               (1, 'alpha', '/r/alpha', 'abc123', 0, 0),
               (2, 'beta', '/r/beta', 'def456', 1, 0);
             INSERT INTO files VALUES
               (1, 1, 'a.rb', 'ruby'), (2, 2, 'blob', NULL);
             INSERT INTO symbols VALUES
               (1, 1, 1, NULL, 'file', 'a.rb', NULL, 'a_rb'),
               (2, 1, 1, 3, 'class', 'Alpha', NULL, 'alpha'),
               (3, 1, 1, 9, 'class', 'Omega', 'Alpha', 'omega'),
               (4, 2, 2, NULL, 'file', 'blob', NULL, 'blob');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn renders_the_global_table_with_totals() {
        let out = global(&conn_with_fixture()).unwrap();
        assert_eq!(
            out,
            "repo            | files | symbols\n\
             ----------------+-------+--------\n\
             alpha           |     1 |       3\n\
             beta            |     1 |       1\n\
             \n\
             repo            | files | symbols\n\
             ----------------+-------+--------\n\
             total (2 repos) |     2 |       4\n"
        );
    }

    #[test]
    fn renders_a_detail_block_per_repository() {
        let out = detail(&conn_with_fixture(), &[1]).unwrap();
        assert_eq!(
            out,
            "Repo name: alpha\nRepo path: /r/alpha\n\
             Head commit: abc123\nFiles: 1\nSymbols: 3\n\
             Symbol kinds:\n  class 2\n  file 1\n"
        );
    }

    #[test]
    fn marks_dirty_repositories_in_the_detail_block() {
        let out = detail(&conn_with_fixture(), &[2]).unwrap();
        assert!(out.contains("Head commit: def456 (dirty)\n"));
    }

    #[test]
    fn separates_multiple_detail_blocks_with_a_blank_line() {
        let out = detail(&conn_with_fixture(), &[1, 2]).unwrap();
        assert!(out.contains("\n\nRepo name: beta\n"));
    }

    #[test]
    fn lists_files_with_their_symbols_indented() {
        let out = files(&conn_with_fixture(), &[1, 2], &[]).unwrap();
        assert_eq!(
            out,
            "a.rb\n  file: a.rb\n  class: Alpha\n  class: Omega\n\
             blob\n  file: blob\n"
        );
    }

    #[test]
    fn filters_the_file_listing_by_kind() {
        let kinds = vec!["class".to_string()];
        let out = files(&conn_with_fixture(), &[1, 2], &kinds).unwrap();
        assert_eq!(out, "a.rb\n  class: Alpha\n  class: Omega\n");
    }

    #[test]
    fn rejects_symbol_kinds_without_any_hit() {
        let kinds = vec!["nope".to_string()];
        let result = files(&conn_with_fixture(), &[1], &kinds);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("kinds: class, file")
        );
    }
}
