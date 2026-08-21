//! Query parsing and the TSV plumbing output.
//!
//! A query is `[repo:] [@kind ...] [term words ...]`, every part
//! optional. The repo atom resolves through the abbreviation map,
//! exact repo names and unique repo-name prefixes; `@kind` sigils
//! resolve against the canonical kind list (unique prefixes work) and
//! OR-combine; the term words are normalized and AND-combine as
//! substring filters on the normalized symbol names.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use rusqlite::Connection;

use crate::lang::Kind;
use crate::normalize::{
    contains_at_boundary, contains_whole_tokens, normalize,
};

/// A parsed query.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedQuery {
    /// The resolved repository ids for the atom, or +nil+ without one.
    pub repo_ids: Option<Vec<i64>>,
    /// The resolved repository name filter for display purposes.
    pub repo_filter: Option<String>,
    /// The resolved symbol kinds of the `@` sigils (OR-combined).
    pub kinds: Vec<String>,
    /// The bare term with all atoms stripped.
    pub term: String,
}

impl ParsedQuery {
    /// The normalized term words of the query.
    ///
    /// These are the AND-combined substring filters the matching runs
    /// on — and the words the pick feed highlights in its display
    /// columns. A word containing a slash is classified as a path
    /// word: slashes are path syntax, so it filters the file path
    /// only. Scope-qualified names spell with `::` (`users_api::
    /// api_v1`), which normalizes to the same key but keeps matching
    /// the qualified symbol name.
    ///
    /// @return the normalized, non-empty term words
    pub fn words(&self) -> Vec<Word> {
        self.term
            .split_whitespace()
            .map(|token| Word {
                norm: normalize(token),
                path_only: token.contains('/'),
            })
            .filter(|word| !word.norm.is_empty())
            .collect()
    }
}

/// One normalized term word with its matching scope.
#[derive(Debug, PartialEq, Eq)]
pub struct Word {
    /// The normalized word text.
    pub norm: String,
    /// Whether the word filters the file path only (it contained a
    /// slash in its raw spelling).
    pub path_only: bool,
}

/// One emitted candidate row.
#[derive(Debug)]
pub struct Row {
    /// The repository name.
    pub repo: String,
    /// The repo-relative file path.
    pub path: String,
    /// The 1-based line, or +nil+ for file rows.
    pub line: Option<i64>,
    /// The symbol kind.
    pub kind: String,
    /// The enclosing scope, or +nil+ at top level.
    pub scope: Option<String>,
    /// The display name.
    pub name: String,
    /// The normalized leaf name.
    pub name_norm: String,
    /// The normalized fully-qualified name (scope included).
    pub qual_norm: String,
    /// The normalized file path (internal, not part of the TSV
    /// contract).
    pub path_norm: String,
}

impl Row {
    /// Render the row as one TSV line of the documented contract
    /// (columns: repo, path, line, kind, scope, name, name_norm,
    /// qual_norm).
    ///
    /// @return the tab-separated line without trailing newline
    pub fn to_tsv(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.repo,
            self.path,
            self.line.map(|line| line.to_string()).unwrap_or_default(),
            self.kind,
            self.scope.as_deref().unwrap_or(""),
            self.name,
            self.name_norm,
            self.qual_norm
        )
    }

    /// Render the row as one pick feed line: the machine columns
    /// (repo, path, line) first, then the display columns fzf shows
    /// (`repo@kind`, scoped name, `path[:LINE]`).
    ///
    /// The matched term parts of the name and path columns are
    /// painted with the given SGR parameters (fzf runs with --ansi),
    /// mirroring what fzf's own matcher would highlight if it were
    /// not disabled. Only words that could have matched a column
    /// light up in it: path words never touch a symbol name, while a
    /// file row's name is its path and takes every word.
    ///
    /// @param words the normalized query words to highlight
    /// @param highlight the ANSI SGR parameters, or empty to disable
    /// @return the tab-separated feed line without trailing newline
    pub fn to_pick(&self, words: &[Word], highlight: &str) -> String {
        let all_words: Vec<&str> =
            words.iter().map(|word| word.norm.as_str()).collect();
        let name_words: Vec<&str> = words
            .iter()
            .filter(|word| !word.path_only)
            .map(|word| word.norm.as_str())
            .collect();

        let name = match self.scope.as_deref() {
            Some(scope) => format!("{scope}::{}", self.name),
            None => self.name.clone(),
        };
        // A file row's name already is the whole path — showing the
        // location column too would just repeat it
        if self.kind == Kind::File.as_str() {
            let name = paint_matches(&name, &all_words, highlight);
            return format!(
                "{}\t{}\t{}\t{}@{}\t{}\t",
                self.repo,
                self.path,
                self.line.map(|line| line.to_string()).unwrap_or_default(),
                self.repo,
                self.kind,
                name
            );
        }

        let name = paint_matches(&name, &name_words, highlight);
        let path = paint_matches(&self.path, &all_words, highlight);
        let location = match self.line {
            Some(line) => format!("{path}:{line}"),
            None => path,
        };
        format!(
            "{}\t{}\t{}\t{}@{}\t{}\t{}",
            self.repo,
            self.path,
            self.line.map(|line| line.to_string()).unwrap_or_default(),
            self.repo,
            self.kind,
            name,
            location
        )
    }
}

/// Paint the matched term parts of a display string.
///
/// The spans come from +normalize::match_spans+, so exactly the
/// token runs the normalized matching hit light up; everything
/// outside them stays untouched.
///
/// @param text the raw display text
/// @param words the normalized query words
/// @param sgr the ANSI SGR parameters, or empty to disable
/// @return the painted display text
fn paint_matches(text: &str, words: &[&str], sgr: &str) -> String {
    if words.is_empty() || sgr.trim().is_empty() {
        return text.to_string();
    }
    let spans = crate::normalize::match_spans(text, words);
    if spans.is_empty() {
        return text.to_string();
    }
    let mut painted = String::new();
    let mut cursor = 0;
    for (start, end) in spans {
        painted.push_str(&text[cursor..start]);
        painted.push_str(&format!("\x1b[{sgr}m{}\x1b[0m", &text[start..end]));
        cursor = end;
    }
    painted.push_str(&text[cursor..]);
    painted
}

/// Parse a query string, resolving the repo atom and kind sigils.
///
/// The repo atom is a `<token>:` prefix followed by whitespace or the
/// end of the input; `Foo::Bar` therefore never parses as an atom.
/// Resolution order: abbreviation map, exact repo name, unique
/// repo-name prefix. `@kind` sigils may appear anywhere in the rest.
///
/// @param conn the open database connection
/// @param abbreviations the configured abbreviation map
/// @param input the raw query string
/// @return the parsed query
/// @raise when an atom or kind sigil does not resolve
pub fn parse(
    conn: &Connection,
    abbreviations: &BTreeMap<String, String>,
    input: &str,
) -> Result<ParsedQuery> {
    let input = input.trim();
    let (repo_ids, repo_filter, rest) = match split_atom(input) {
        Some((atom, rest)) => {
            let (name, ids) = resolve_repos(conn, abbreviations, atom)?;
            (Some(ids), Some(name), rest)
        }
        None => (None, None, input),
    };

    // Pull the @kind sigils out of the term words
    let mut kinds = Vec::new();
    let mut words = Vec::new();
    for token in rest.split_whitespace() {
        match token.strip_prefix('@') {
            Some(sigil) => kinds.push(resolve_kind(sigil)?),
            None => words.push(token),
        }
    }
    kinds.sort();
    kinds.dedup();

    Ok(ParsedQuery {
        repo_ids,
        repo_filter,
        kinds,
        term: words.join(" "),
    })
}

/// Resolve a kind sigil to its canonical kind name.
///
/// Resolution order: exact kind name, unique kind-name prefix — the
/// same feel as the repo atom resolution.
///
/// @param sigil the kind token without the `@` prefix
/// @return the canonical kind name
/// @raise when the sigil is empty, unknown or ambiguous
fn resolve_kind(sigil: &str) -> Result<String> {
    let needle = sigil.to_ascii_lowercase();
    if let Some(kind) = Kind::ALL.iter().find(|kind| **kind == needle) {
        return Ok((*kind).to_string());
    }
    let matches: Vec<&str> = Kind::ALL
        .iter()
        .copied()
        .filter(|kind| !needle.is_empty() && kind.starts_with(&needle))
        .collect();
    match matches.as_slice() {
        [kind] => Ok((*kind).to_string()),
        [] => {
            bail!("unknown kind '@{sigil}' (kinds: {})", Kind::ALL.join(", "))
        }
        many => {
            bail!("ambiguous kind '@{sigil}' (matches: {})", many.join(", "))
        }
    }
}

/// Resolve a repository token to its repository ids.
///
/// Resolution order: abbreviation map, exact repo name, repo-name
/// prefix — the same rules a query atom follows.
///
/// @param conn the open database connection
/// @param abbreviations the configured abbreviation map
/// @param token the repository name, abbreviation or prefix
/// @return the resolved repository name and its ids
/// @raise when the token does not resolve to any repository
pub fn resolve_repos(
    conn: &Connection,
    abbreviations: &BTreeMap<String, String>,
    token: &str,
) -> Result<(String, Vec<i64>)> {
    let name = abbreviations
        .get(token)
        .cloned()
        .unwrap_or_else(|| token.to_string());

    // Exact repo-name match first, unique prefix as fallback
    let mut ids = repo_ids_by_name(conn, &name, false)?;
    if ids.is_empty() {
        ids = repo_ids_by_name(conn, &name, true)?;
    }
    if ids.is_empty() {
        let known = known_repo_names(conn)?;
        bail!("unknown repository '{token}' (known: {})", known.join(", "));
    }
    Ok((name, ids))
}

/// Split a leading repo atom off the query string.
///
/// @param input the trimmed query string
/// @return the atom and the remaining term, or +nil+ without an atom
fn split_atom(input: &str) -> Option<(&str, &str)> {
    let (atom, rest) = input.split_once(':')?;
    if atom.is_empty()
        || !atom
            .chars()
            .all(|chr| chr.is_ascii_alphanumeric() || chr == '-' || chr == '_')
    {
        return None;
    }
    // Require whitespace (or end) after the colon so `Foo::Bar` stays
    // a plain term
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some((atom, rest.trim()))
}

/// Look up repository ids by name.
///
/// @param conn the open database connection
/// @param name the repository name or name prefix
/// @param prefix whether to match by prefix instead of exactly
/// @return the matching repository ids
fn repo_ids_by_name(
    conn: &Connection,
    name: &str,
    prefix: bool,
) -> Result<Vec<i64>> {
    let (sql, pattern) = if prefix {
        (
            "SELECT id FROM repos WHERE name LIKE ?1 ESCAPE '\\'",
            format!("{}%", like_escape(name)),
        )
    } else {
        ("SELECT id FROM repos WHERE name = ?1", name.to_string())
    };
    let ids = conn
        .prepare(sql)?
        .query_map([pattern], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// List all known repository names.
///
/// @param conn the open database connection
/// @return the sorted repository names
fn known_repo_names(conn: &Connection) -> Result<Vec<String>> {
    let names = conn
        .prepare("SELECT DISTINCT name FROM repos ORDER BY name")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(names)
}

/// Escape LIKE wildcards in user input.
///
/// @param raw the raw user input
/// @return the escaped LIKE pattern fragment
fn like_escape(raw: &str) -> String {
    raw.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Run a parsed query and collect the candidate rows.
///
/// Words spelled with a slash filter the file path only; the rest
/// matches the qualified name or the path. Every occurrence must
/// start at a token boundary. Rows are ranked by specificity first
/// (symbols matching every name word with their qualified name beat
/// path-assisted matches), then whole-token matches beat
/// boundary-prefix ones, then by match quality of the whole
/// normalized term (exact, prefix, contains), then by kind relevance
/// (+Kind::RANKING+), then by target length (denser matches first),
/// then ordered by (repo, path, line). A file row is dropped once a
/// specific symbol hit from the same file is listed — it would only
/// echo that symbol's path.
///
/// @param conn the open database connection
/// @param parsed the parsed query
/// @param limit the maximum number of rows, or +nil+ for all
/// @return the ranked candidate rows
pub fn run(
    conn: &Connection,
    parsed: &ParsedQuery,
    limit: Option<usize>,
) -> Result<Vec<Row>> {
    let mut sql = String::from(
        "SELECT repos.name, files.path, symbols.line,
                symbols.kind, symbols.scope, symbols.name,
                symbols.name_norm, symbols.qual_norm, files.path_norm
         FROM symbols
         JOIN files ON files.id = symbols.file_id
         JOIN repos ON repos.id = symbols.repo_id
         WHERE 1 = 1",
    );
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(ids) = &parsed.repo_ids {
        let marks = vec!["?"; ids.len()].join(", ");
        sql.push_str(&format!(" AND symbols.repo_id IN ({marks})"));
        for id in ids {
            params.push(Box::new(*id));
        }
    }

    // The @kind sigils OR-combine into one kind set
    if !parsed.kinds.is_empty() {
        let marks = vec!["?"; parsed.kinds.len()].join(", ");
        sql.push_str(&format!(" AND symbols.kind IN ({marks})"));
        for kind in &parsed.kinds {
            params.push(Box::new(kind.clone()));
        }
    }

    // Space-separated words AND-combine (like fzf terms do): every
    // normalized word must occur in the fully-qualified normalized
    // name or in the file's normalized path — a symbol and its file
    // path belong together, so `lib/remote EntityRef` finds the
    // class through one path word and one name word. Path words
    // (spelled with a slash) filter the file path only, so `spec/`
    // never drowns in symbols merely named spec. Occurrences must
    // start at a token boundary (the string start or right after an
    // underscore): `pain` finds painless, `ai` does not
    let words = parsed.words();
    for word in &words {
        let prefix = format!("{}%", like_escape(&word.norm));
        let inner = format!("%\\_{}%", like_escape(&word.norm));
        if word.path_only {
            sql.push_str(
                " AND (files.path_norm LIKE ? ESCAPE '\\'
                    OR files.path_norm LIKE ? ESCAPE '\\')",
            );
            params.push(Box::new(prefix));
            params.push(Box::new(inner));
        } else {
            sql.push_str(
                " AND (symbols.qual_norm LIKE ? ESCAPE '\\'
                    OR symbols.qual_norm LIKE ? ESCAPE '\\'
                    OR files.path_norm LIKE ? ESCAPE '\\'
                    OR files.path_norm LIKE ? ESCAPE '\\')",
            );
            params.push(Box::new(prefix.clone()));
            params.push(Box::new(inner.clone()));
            params.push(Box::new(prefix));
            params.push(Box::new(inner));
        }
    }

    // Rank whole-term matches: leaf name hits (exact, prefix) first,
    // qualified-name hits second, mere containment last — so `user`
    // surfaces `User` above `UsersApi` and both above `Foo::User#bar`.
    // Ties break by kind relevance (see +Kind::RANKING+): concrete
    // code symbols feed first, file-row echoes last — then by target
    // length: with a fixed word count, a shorter qualified name means
    // the matched tokens make up more of it (density). Path words
    // stay out of the ranking term: they narrow the tree, so
    // `spec/ user` still ranks exact `user` names first.
    let term_norm = words
        .iter()
        .filter(|word| !word.path_only)
        .map(|word| word.norm.as_str())
        .collect::<Vec<_>>()
        .join("_");
    if term_norm.is_empty() {
        sql.push_str(" ORDER BY repos.name, files.path, symbols.line");
    } else {
        let kind_case = Kind::RANKING
            .iter()
            .enumerate()
            .map(|(rank, kind)| format!("WHEN '{kind}' THEN {rank}"))
            .collect::<Vec<_>>()
            .join(" ");
        sql.push_str(&format!(
            " ORDER BY CASE
                 WHEN symbols.name_norm = ? THEN 0
                 WHEN symbols.name_norm LIKE ? ESCAPE '\\' THEN 1
                 WHEN symbols.qual_norm = ? THEN 2
                 WHEN symbols.qual_norm LIKE ? ESCAPE '\\' THEN 3
                 ELSE 4 END,
               CASE symbols.kind {kind_case} ELSE 99 END,
               length(symbols.qual_norm),
               repos.name, files.path, symbols.line"
        ));
        let prefix = format!("{}%", like_escape(&term_norm));
        params.push(Box::new(term_norm.clone()));
        params.push(Box::new(prefix.clone()));
        params.push(Box::new(term_norm.clone()));
        params.push(Box::new(prefix));
    }
    // With term words the specificity post-processing below decides
    // the final cut, so the limit applies after it, not in SQL
    if let Some(limit) = limit
        && words.is_empty()
    {
        sql.push_str(&format!(" LIMIT {limit}"));
    }

    let mut rows: Vec<Row> = conn
        .prepare(&sql)?
        .query_map(
            rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
            |row| {
                Ok(Row {
                    repo: row.get(0)?,
                    path: row.get(1)?,
                    line: row.get(2)?,
                    kind: row.get(3)?,
                    scope: row.get(4)?,
                    name: row.get(5)?,
                    name_norm: row.get(6)?,
                    qual_norm: row.get(7)?,
                    path_norm: row.get(8)?,
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;

    // Specificity pass: a specific hit is a symbol whose qualified
    // name alone covers every name word; path-assisted rows only got
    // in via their file path. Specific hits always rank first, and a
    // file row is dropped once a specific hit from the same file is
    // listed — that symbol row already points at the file, the file
    // row is a redundant echo. File rows of other files stay: they
    // are the only pointer to their file. Path words never count:
    // they filter the tree, so path-only searches (eg. `spec/`) are
    // file searches and keep all their file rows.
    if !words.is_empty() {
        let name_words: Vec<&str> = words
            .iter()
            .filter(|word| !word.path_only)
            .map(|word| word.norm.as_str())
            .collect();
        let specific = |row: &Row| {
            !name_words.is_empty()
                && row.kind != Kind::File.as_str()
                && name_words
                    .iter()
                    .all(|word| contains_at_boundary(&row.qual_norm, word))
        };
        if !name_words.is_empty() {
            let covered: std::collections::HashSet<(String, String)> = rows
                .iter()
                .filter(|row| specific(row))
                .map(|row| (row.repo.clone(), row.path.clone()))
                .collect();
            rows.retain(|row| {
                row.kind != Kind::File.as_str()
                    || !covered.contains(&(row.repo.clone(), row.path.clone()))
            });
        }
        // Whole-token tier: rows where every word matches a complete
        // token run (`ai` the token ai, never the start of aid) beat
        // boundary-prefix matches. Stable sort per tier — the SQL
        // rank order survives within each group
        let whole = |row: &Row| {
            words.iter().all(|word| match word.path_only {
                true => contains_whole_tokens(&row.path_norm, &word.norm),
                false => {
                    contains_whole_tokens(&row.qual_norm, &word.norm)
                        || contains_whole_tokens(&row.path_norm, &word.norm)
                }
            })
        };
        rows.sort_by_key(|row| (!specific(row), !whole(row)));
        if let Some(limit) = limit {
            rows.truncate(limit);
        }
    }
    Ok(rows)
}

/// Collect the landing rows: one configured landing file per repo.
///
/// This is the empty-input state of `kartei pick` — instead of the
/// whole index, every repository contributes just its landing file
/// row (eg. its README.md), acting as a project browser.
///
/// @param conn the open database connection
/// @param landing the repo-relative landing file path
/// @return the landing rows in repo-name order
pub fn landing(conn: &Connection, landing: &str) -> Result<Vec<Row>> {
    let rows = conn
        .prepare(
            "SELECT repos.name, files.path, symbols.line,
                    symbols.kind, symbols.scope, symbols.name,
                    symbols.name_norm, symbols.qual_norm, files.path_norm
             FROM symbols
             JOIN files ON files.id = symbols.file_id
             JOIN repos ON repos.id = symbols.repo_id
             WHERE symbols.kind = 'file' AND files.path = ?1
             ORDER BY repos.name",
        )?
        .query_map([landing], |row| {
            Ok(Row {
                repo: row.get(0)?,
                path: row.get(1)?,
                line: row.get(2)?,
                kind: row.get(3)?,
                scope: row.get(4)?,
                name: row.get(5)?,
                name_norm: row.get(6)?,
                qual_norm: row.get(7)?,
                path_norm: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open an in-memory database with the schema and two repos.
    fn conn_with_repos() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE repos (id INTEGER PRIMARY KEY, name TEXT,
               root TEXT, head TEXT, dirty INTEGER, indexed_at INTEGER);
             INSERT INTO repos (id, name, root, dirty, indexed_at)
             VALUES (1, 'acme-api', '/r/acme-api', 0, 0),
                    (2, 'blog-engine', '/r/blog-engine', 0, 0);",
        )
        .unwrap();
        conn
    }

    /// The empty abbreviation map.
    fn no_abbrevs() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[test]
    fn parses_a_bare_term() {
        let conn = conn_with_repos();
        let parsed = parse(&conn, &no_abbrevs(), "UserPolicy").unwrap();
        assert_eq!(parsed.term, "UserPolicy");
    }

    #[test]
    fn keeps_scope_resolution_terms_intact() {
        let conn = conn_with_repos();
        let parsed = parse(&conn, &no_abbrevs(), "Foo::Bar").unwrap();
        assert_eq!(parsed.repo_ids, None);
    }

    #[test]
    fn resolves_exact_repo_atoms() {
        let conn = conn_with_repos();
        let parsed = parse(&conn, &no_abbrevs(), "blog-engine: User").unwrap();
        assert_eq!(parsed.repo_ids, Some(vec![2]));
    }

    #[test]
    fn resolves_abbreviation_atoms() {
        let conn = conn_with_repos();
        let mut abbrevs = no_abbrevs();
        abbrevs.insert("aa".to_string(), "acme-api".to_string());
        let parsed = parse(&conn, &abbrevs, "aa: User").unwrap();
        assert_eq!(parsed.repo_ids, Some(vec![1]));
    }

    #[test]
    fn resolves_prefix_atoms() {
        let conn = conn_with_repos();
        let parsed = parse(&conn, &no_abbrevs(), "acme: User").unwrap();
        assert_eq!(parsed.repo_ids, Some(vec![1]));
    }

    #[test]
    fn strips_the_atom_from_the_term() {
        let conn = conn_with_repos();
        let parsed = parse(&conn, &no_abbrevs(), "acme-api: User").unwrap();
        assert_eq!(parsed.term, "User");
    }

    #[test]
    fn rejects_unknown_atoms() {
        let conn = conn_with_repos();
        assert!(parse(&conn, &no_abbrevs(), "nope: User").is_err());
    }

    #[test]
    fn resolves_exact_kind_sigils() {
        let conn = conn_with_repos();
        let parsed = parse(&conn, &no_abbrevs(), "@class user").unwrap();
        assert_eq!(parsed.kinds, vec!["class".to_string()]);
    }

    #[test]
    fn resolves_kind_sigil_prefixes() {
        let conn = conn_with_repos();
        let parsed = parse(&conn, &no_abbrevs(), "@meth user").unwrap();
        assert_eq!(parsed.kinds, vec!["method".to_string()]);
    }

    #[test]
    fn combines_multiple_kind_sigils() {
        let conn = conn_with_repos();
        let parsed = parse(&conn, &no_abbrevs(), "@class @module").unwrap();
        assert_eq!(
            parsed.kinds,
            vec!["class".to_string(), "module".to_string()]
        );
    }

    #[test]
    fn strips_kind_sigils_from_the_term() {
        let conn = conn_with_repos();
        let parsed =
            parse(&conn, &no_abbrevs(), "acme-api: @class User").unwrap();
        assert_eq!(parsed.term, "User");
    }

    #[test]
    fn rejects_ambiguous_kind_sigils() {
        let conn = conn_with_repos();
        let result = parse(&conn, &no_abbrevs(), "@m user");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("matches: module, method")
        );
    }

    #[test]
    fn rejects_unknown_kind_sigils() {
        let conn = conn_with_repos();
        assert!(parse(&conn, &no_abbrevs(), "@nope user").is_err());
    }

    #[test]
    fn rejects_empty_kind_sigils() {
        let conn = conn_with_repos();
        assert!(parse(&conn, &no_abbrevs(), "@ user").is_err());
    }

    /// Extend the repo fixture with files and a few symbols.
    fn conn_with_symbols() -> Connection {
        let conn = conn_with_repos();
        conn.execute_batch(
            "CREATE TABLE files (id INTEGER PRIMARY KEY,
               repo_id INTEGER, path TEXT, path_norm TEXT,
               mtime_ns INTEGER, size INTEGER, lang TEXT);
             CREATE TABLE symbols (id INTEGER PRIMARY KEY,
               repo_id INTEGER, file_id INTEGER, line INTEGER,
               kind TEXT, name TEXT, scope TEXT, name_norm TEXT,
               qual_norm TEXT);
             INSERT INTO files VALUES
               (1, 2, 'lib/user.rb', 'lib_user_rb', 0, 0, 'ruby'),
               (2, 2, 'README.md', 'readme_md', 0, 0, NULL),
               (3, 2, 'docs/user_guide.md', 'doc_user_guide_md',
                0, 0, NULL),
               (4, 2, 'notes/user.md', 'note_user_md', 0, 0, NULL);
             INSERT INTO symbols VALUES
               (1, 2, 1, 3, 'class', 'User', NULL, 'user', 'user'),
               (2, 2, 1, 9, 'class', 'UsersApi', NULL,
                'user_api', 'user_api'),
               (3, 2, 1, 12, 'method', 'user_name', NULL,
                'user_name', 'user_name'),
               (4, 2, 1, 15, 'const', 'Find', 'UsersApi::ApiV1',
                'find', 'user_api_api_v_1_find'),
               (5, 2, 2, NULL, 'file', 'README.md', NULL,
                'readme_md', 'readme_md'),
               (6, 2, 1, NULL, 'file', 'lib/user.rb', NULL,
                'lib_user_rb', 'lib_user_rb'),
               (7, 2, 1, 20, 'method', 'helper', NULL,
                'helper', 'helper'),
               (8, 2, 1, 2, 'method', 'user', NULL, 'user', 'user'),
               (9, 2, 3, NULL, 'file', 'docs/user_guide.md', NULL,
                'doc_user_guide_md', 'doc_user_guide_md'),
               (10, 2, 4, NULL, 'file', 'notes/user.md', NULL,
                'note_user_md', 'note_user_md'),
               (11, 2, 1, 5, 'method', 'sort_helper', NULL,
                'sort_helper', 'sort_helper'),
               (12, 2, 1, 3, 'method', 'sorter_x', NULL,
                'sorter_x', 'sorter_x');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn filters_rows_by_kind() {
        let conn = conn_with_symbols();
        let parsed = parse(&conn, &no_abbrevs(), "@method user").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        // The name hits rank above the merely path-assisted methods,
        // which order by target length (shortest first)
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["user", "user_name", "helper", "sorter_x", "sort_helper"]
        );
    }

    #[test]
    fn ranks_exact_matches_before_containment() {
        let conn = conn_with_symbols();
        let parsed = parse(&conn, &no_abbrevs(), "user").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        // Exact leaf hits first (the class beats the method on equal
        // rank tiers — classes lead the kind relevance), leaf
        // prefixes next, the qualified-only hit after, the
        // path-assisted ones last — the lib/user.rb file row drops
        // behind its specific symbols, while the guide and note file
        // rows are the only pointers to their files and stay (file
        // rows outrank methods now), denser (shorter) targets first
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec![
                "User",
                "user",
                "UsersApi",
                "user_name",
                "Find",
                "notes/user.md",
                "docs/user_guide.md",
                "helper",
                "sorter_x",
                "sort_helper"
            ]
        );
    }

    #[test]
    fn rejects_mid_token_matches() {
        let conn = conn_with_symbols();
        // `ser` only occurs inside the `user` token — matches must
        // start at a token boundary, so nothing is found
        let parsed = parse(&conn, &no_abbrevs(), "ser").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        assert!(
            rows.is_empty(),
            "mid-token fragments must not match, got: {rows:?}"
        );
    }

    #[test]
    fn ranks_whole_token_matches_first() {
        let conn = conn_with_symbols();
        // `sort` is a complete token of sort_helper but only a
        // prefix of sorter_x's leading token — the whole-token match
        // wins despite the longer name
        let parsed = parse(&conn, &no_abbrevs(), "@method sort").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["sort_helper", "sorter_x"]
        );
    }

    #[test]
    fn ranks_denser_file_targets_first() {
        let conn = conn_with_symbols();
        let parsed = parse(&conn, &no_abbrevs(), "user").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        let paths: Vec<&str> = rows
            .iter()
            .filter(|row| row.kind == "file")
            .map(|row| row.path.as_str())
            .collect();
        // Both file rows match equally — the shorter target carries
        // a denser match and ranks first
        assert_eq!(paths, vec!["notes/user.md", "docs/user_guide.md"]);
    }

    #[test]
    fn keeps_file_rows_of_files_without_specific_hits() {
        let conn = conn_with_symbols();
        let parsed = parse(&conn, &no_abbrevs(), "user").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        // The guide matched only through its path — no symbol row
        // points at it, so its file row must survive the drop
        assert!(
            rows.iter()
                .any(|row| row.kind == "file"
                    && row.path == "docs/user_guide.md"),
            "file rows without specific same-file hits must stay"
        );
    }

    #[test]
    fn drops_file_rows_echoing_specific_same_file_hits() {
        let conn = conn_with_symbols();
        let parsed = parse(&conn, &no_abbrevs(), "user").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        // The user symbols already point at lib/user.rb — its own
        // file row is a redundant echo
        assert!(
            !rows
                .iter()
                .any(|row| row.kind == "file" && row.path == "lib/user.rb"),
            "file rows echoing specific same-file hits must drop"
        );
    }

    #[test]
    fn keeps_file_rows_for_path_only_searches() {
        let conn = conn_with_symbols();
        let parsed = parse(&conn, &no_abbrevs(), "lib").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        // No symbol covers `lib` with its name, so nothing is
        // specific and the file row stays in the listing
        assert!(
            rows.iter()
                .any(|row| row.kind == "file" && row.path == "lib/user.rb"),
            "path-only searches must keep file rows"
        );
    }

    #[test]
    fn matches_words_against_the_qualified_name() {
        let conn = conn_with_symbols();
        // The scope spelling uses `::` — it normalizes to the same
        // key a slash would, but keeps matching the qualified name
        let parsed =
            parse(&conn, &no_abbrevs(), "users_api::api_v1 find").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["Find"]
        );
    }

    #[test]
    fn slash_words_do_not_match_qualified_names() {
        let conn = conn_with_symbols();
        // The Find const carries users_api/api_v1 in its scope but
        // not in its file path — a slash word must not reach it
        let parsed =
            parse(&conn, &no_abbrevs(), "users_api/api_v1 find").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        assert!(
            rows.is_empty(),
            "slash words must filter the path only, got: {rows:?}"
        );
    }

    #[test]
    fn slash_words_narrow_the_tree_for_name_words() {
        let conn = conn_with_symbols();
        // `lib/` narrows to lib/user.rb, `helper` finds the methods
        // there by name (exact leaf first, the boundary-anchored
        // second token of sort_helper after) — the specific hits
        // drop the file row
        let parsed = parse(&conn, &no_abbrevs(), "lib/ helper").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["helper", "sort_helper"]
        );
    }

    #[test]
    fn keeps_file_rows_for_pure_path_word_queries() {
        let conn = conn_with_symbols();
        // A query of path words alone is a file search: nothing can
        // be specific, so the file row stays listed
        let parsed = parse(&conn, &no_abbrevs(), "lib/user").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        assert!(
            rows.iter()
                .any(|row| row.kind == "file" && row.path == "lib/user.rb"),
            "pure path-word searches must keep file rows"
        );
    }

    #[test]
    fn mixes_path_and_symbol_words() {
        let conn = conn_with_symbols();
        // `lib` only occurs in the file path, `find` only in the
        // qualified symbol name — together they hit the symbol
        let parsed = parse(&conn, &no_abbrevs(), "lib find").unwrap();
        let rows = run(&conn, &parsed, None).unwrap();
        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["Find"]
        );
    }

    #[test]
    fn collects_landing_rows_per_repository() {
        let conn = conn_with_symbols();
        let rows = landing(&conn, "README.md").unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.repo.as_str(), row.kind.as_str()))
                .collect::<Vec<_>>(),
            vec![("blog-engine", "file")]
        );
    }

    #[test]
    fn renders_empty_line_and_scope_as_empty_fields() {
        let row = Row {
            repo: "a".into(),
            path: "b.rb".into(),
            line: None,
            kind: "file".into(),
            scope: None,
            name: "b.rb".into(),
            name_norm: "b_rb".into(),
            qual_norm: "b_rb".into(),
            path_norm: "b_rb".into(),
        };
        assert_eq!(row.to_tsv(), "a\tb.rb\t\tfile\t\tb.rb\tb_rb\tb_rb");
    }

    #[test]
    fn renders_file_rows_without_a_location_echo() {
        let row = Row {
            repo: "a".into(),
            path: "config/assets.yml".into(),
            line: None,
            kind: "file".into(),
            scope: None,
            name: "config/assets.yml".into(),
            name_norm: "config_asset_yml".into(),
            qual_norm: "config_asset_yml".into(),
            path_norm: "config_asset_yml".into(),
        };
        assert_eq!(
            row.to_pick(&[], ""),
            "a\tconfig/assets.yml\t\ta@file\tconfig/assets.yml\t"
        );
    }

    #[test]
    fn renders_pick_feed_lines_with_display_columns() {
        let row = Row {
            repo: "a".into(),
            path: "user.rb".into(),
            line: Some(3),
            kind: "class".into(),
            scope: Some("Api".into()),
            name: "User".into(),
            name_norm: "user".into(),
            qual_norm: "api_user".into(),
            path_norm: "user_rb".into(),
        };
        assert_eq!(
            row.to_pick(&[], ""),
            "a\tuser.rb\t3\ta@class\tApi::User\tuser.rb:3"
        );
    }

    #[test]
    fn paints_matched_parts_in_the_pick_feed() {
        let row = Row {
            repo: "a".into(),
            path: "user.rb".into(),
            line: Some(3),
            kind: "class".into(),
            scope: Some("Api".into()),
            name: "User".into(),
            name_norm: "user".into(),
            qual_norm: "api_user".into(),
            path_norm: "user_rb".into(),
        };
        let words = vec![Word {
            norm: "user".to_string(),
            path_only: false,
        }];
        assert_eq!(
            row.to_pick(&words, "31"),
            "a\tuser.rb\t3\ta@class\tApi::\x1b[31mUser\x1b[0m\t\
             \x1b[31muser\x1b[0m.rb:3"
        );
    }

    #[test]
    fn keeps_symbol_names_plain_for_path_words() {
        let row = Row {
            repo: "a".into(),
            path: "user.rb".into(),
            line: Some(3),
            kind: "class".into(),
            scope: None,
            name: "User".into(),
            name_norm: "user".into(),
            qual_norm: "user".into(),
            path_norm: "user_rb".into(),
        };
        // A path word never matched the symbol name, so only the
        // path column lights up
        let words = vec![Word {
            norm: "user".to_string(),
            path_only: true,
        }];
        assert_eq!(
            row.to_pick(&words, "31"),
            "a\tuser.rb\t3\ta@class\tUser\t\x1b[31muser\x1b[0m.rb:3"
        );
    }

    #[test]
    fn paints_file_row_names_with_path_words() {
        let row = Row {
            repo: "a".into(),
            path: "lib/user.rb".into(),
            line: None,
            kind: "file".into(),
            scope: None,
            name: "lib/user.rb".into(),
            name_norm: "lib_user_rb".into(),
            qual_norm: "lib_user_rb".into(),
            path_norm: "lib_user_rb".into(),
        };
        // A file row's name is its path, so path words paint it
        let words = vec![Word {
            norm: "lib_user".to_string(),
            path_only: true,
        }];
        assert_eq!(
            row.to_pick(&words, "31"),
            "a\tlib/user.rb\t\ta@file\t\x1b[31mlib/user\x1b[0m.rb\t"
        );
    }

    #[test]
    fn keeps_the_pick_feed_plain_without_a_highlight() {
        let row = Row {
            repo: "a".into(),
            path: "user.rb".into(),
            line: Some(3),
            kind: "class".into(),
            scope: None,
            name: "User".into(),
            name_norm: "user".into(),
            qual_norm: "user".into(),
            path_norm: "user_rb".into(),
        };
        let words = vec![Word {
            norm: "user".to_string(),
            path_only: false,
        }];
        assert_eq!(
            row.to_pick(&words, ""),
            "a\tuser.rb\t3\ta@class\tUser\tuser.rb:3"
        );
    }

    #[test]
    fn derives_normalized_query_words() {
        let conn = conn_with_repos();
        let parsed = parse(&conn, &no_abbrevs(), "@class spec/ find").unwrap();
        assert_eq!(
            parsed.words(),
            vec![
                Word {
                    norm: "spec".to_string(),
                    path_only: true
                },
                Word {
                    norm: "find".to_string(),
                    path_only: false
                }
            ]
        );
    }

    #[test]
    fn classifies_scope_spellings_as_name_words() {
        let conn = conn_with_repos();
        let parsed = parse(&conn, &no_abbrevs(), "users_api::api_v1").unwrap();
        assert_eq!(
            parsed.words(),
            vec![Word {
                norm: "user_api_api_v_1".to_string(),
                path_only: false
            }]
        );
    }
}
