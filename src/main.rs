//! kartei — a personal code-symbol index and search CLI.
//!
//! The indexer discovers git repositories under configured start
//! paths, extracts code symbols with tree-sitter and keeps them in one
//! SQLite database for incremental, cron-friendly refreshes. The query
//! side emits TSV candidates (plumbing) or drives fzf directly
//! (porcelain), printing `path[:LINE]` targets ready for `vim`.

#![warn(missing_docs)]

mod config;
mod db;
mod discover;
mod git;
mod indexer;
mod lang;
mod normalize;
mod pick;
mod query;
mod stats;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// The version string shown by --version, extended with the build
/// date and build number stamped by the build script.
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (built ",
    env!("KARTEI_BUILD_DATE"),
    ", ",
    env!("KARTEI_BUILD_NUMBER"),
    ")"
);

/// The extended --help footer of the index subcommand.
const INDEX_LONG_HELP: &str = "\
Discovers git repositories under the configured start paths and
indexes their tracked files incrementally: unchanged repositories
are skipped via a HEAD/dirty fast path, unchanged files via their
mtime/size. A lock next to the database prevents concurrent runs.

Use --full after configuration changes (eg. index.ignores), so
clean repositories pick them up too.";

/// The shared QUERY grammar block of the query/pick --help footers.
///
/// @return the rendered grammar documentation
fn grammar_help() -> String {
    format!(
        "\
QUERY is `[repo:] [@kind ...] [term words ...]`, every part
optional. The repo atom (name, abbreviation or unique prefix) needs
whitespace (or end of input) after the colon, so Foo::Bar is never
mistaken for one. Space-separated term words AND-combine, each
matching the normalized qualified name (scope included) or the file
path the symbol lives in; matches must start at a token boundary
(`pain` finds painless, `ai` does not). Words are case-folded and
singularized on both sides, so `businessCase` == `business_cases`.
A word spelled with a slash filters the file path only (`spec/`,
`lib/remote`) - fragments match across component boundaries;
scope-qualified names spell with `::` (`users_api::api_v1`).

The @kind sigils restrict the symbol kind; several OR-combine and
unique prefixes work (eg. @meth). All sigils:

  {}",
        wrap_sigils(lang::Kind::ALL)
    )
}

/// Render the @-prefixed kind sigils, wrapped at the help width.
///
/// @param kinds the canonical kind names
/// @return the wrapped sigil lines
fn wrap_sigils(kinds: &[&str]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for sigil in kinds.iter().map(|kind| format!("@{kind}")) {
        match lines.last_mut() {
            Some(line) if line.len() + sigil.len() < 60 => {
                line.push(' ');
                line.push_str(&sigil);
            }
            _ => lines.push(sigil),
        }
    }
    lines.join("\n  ")
}

/// Build the extended --help footer of the query subcommand.
///
/// @return the rendered help footer
fn query_long_help() -> String {
    format!(
        "\
Emits one candidate per line, tab-separated, in reverse relevance
order: the best hit prints last, so it ends up next to your shell
prompt. Ranking: specificity first (symbols matching every name
word by qualified name beat rows matched only through their file
path; a specific hit also drops its own file's plain file row, a
redundant echo - slash-spelled path words never count, they narrow
the tree), then whole-token matches (every word a complete token)
beat boundary-prefix ones, then whole-term match quality (exact,
prefix, contains), then kind relevance (class module file method
func smethod const target arg heading env key image), then target
length (denser matches first), then (repo, path, line) order:

  repo       repository name
  path       repo-relative file path
  line       1-based line number, empty for file rows
  kind       one of: {}
  scope      enclosing context (eg. Foo::Bar), empty at top level
  name       display name of the symbol
  name_norm  normalized leaf name (snake_case folded)
  qual_norm  normalized fully-qualified name, the matching form

This contract is versioned with the tool - wrappers may rely on it.

{}",
        lang::Kind::ALL.join(" "),
        grammar_help()
    )
}

/// Build the extended --help footer of the pick subcommand.
///
/// @return the rendered help footer
fn pick_long_help() -> String {
    format!(
        "\
Runs fzf as a pure display: every keystroke re-queries the index
with the grammar below, so the list always shrinks to what matches
and the pointer snaps back to the best hit next to the prompt.
The matched term parts light up in the list (pick.match_highlight,
dark red by default). An empty input shows the landing view - one
pick.landing file (eg. README.md) per repository. The selection is
printed as
`<absolute-path>[:LINE]` - ready for `vim path/to/file:9`. When one
of the configured pick.expect keys confirmed the selection, the key
is prepended: `<key>\\t<absolute-path>[:LINE]`. Escape clears the
current input back to the landing view; escape on an empty input
aborts - an aborted pick exits with code 130 and no output.

{}",
        grammar_help()
    )
}

/// The extended --help footer of the stats subcommand.
const STATS_LONG_HELP: &str = "\
Without REPO: one aligned table row per repository
(repo | files | symbols) plus a totals row. With REPO (a repository
name, abbreviation or prefix): a detail block with the repository
root, indexed head commit, counts and a symbol-kind breakdown.

--files lists the indexed files instead, each followed by its
symbols as two-space indented `kind: name` lines; --symbols=KIND,..
restricts that listing to the given kinds (and implies --files).";

/// The extended --help footer of the init subcommand.
const INIT_LONG_HELP: &str = "\
Writes a fully commented example configuration to the --config path,
or to the XDG default location (~/.config/kartei/config.toml).
Existing files are only overwritten with --force.";

/// The top-level command-line interface.
#[derive(Parser)]
#[command(name = "kartei", version = VERSION, about)]
struct Cli {
    /// Path to the configuration file.
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Path to the index database.
    #[arg(long, global = true, value_name = "FILE")]
    db: Option<PathBuf>,
    /// The subcommand to run.
    #[command(subcommand)]
    command: Command,
}

/// All kartei subcommands.
#[derive(Subcommand)]
enum Command {
    /// Index all repositories incrementally.
    #[command(after_long_help = INDEX_LONG_HELP)]
    Index {
        /// Ignore stored state and re-extract every file.
        #[arg(long)]
        full: bool,
    },
    /// Emit candidate symbols as TSV (plumbing).
    #[command(after_long_help = query_long_help())]
    Query {
        /// The query: `[repo-or-abbrev:] [term]`.
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
        /// Maximum number of rows to emit.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Interactively pick a symbol via fzf (porcelain).
    #[command(after_long_help = pick_long_help())]
    Pick {
        /// The query: `[repo-or-abbrev:] [term]`.
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
    },
    /// Show index statistics, globally or for one repository.
    #[command(after_long_help = STATS_LONG_HELP)]
    Stats {
        /// The repository name, abbreviation or prefix.
        repo: Option<String>,
        /// List the indexed files with their symbols instead.
        #[arg(long, requires = "repo")]
        files: bool,
        /// Restrict the file listing to these symbol kinds.
        #[arg(
            long,
            value_name = "KIND,..",
            requires = "repo",
            value_delimiter = ',',
            require_equals = true
        )]
        symbols: Vec<String>,
    },
    /// Write an example configuration file.
    #[command(after_long_help = INIT_LONG_HELP)]
    Init {
        /// Overwrite an existing configuration file.
        #[arg(long)]
        force: bool,
    },
    /// Emit the pick feed for one query state (internal helper).
    #[command(hide = true)]
    Candidates {
        /// The current query string.
        query: Option<String>,
    },
    /// Render the preview for one candidate (internal helper).
    #[command(hide = true)]
    Preview {
        /// The repository name.
        repo: String,
        /// The repo-relative file path.
        path: String,
        /// The 1-based symbol line; empty for file rows (fzf always
        /// passes the placeholder, even when the column is blank).
        line: Option<String>,
    },
}

/// Program entry point: dispatch the parsed subcommand.
fn main() {
    let cli = Cli::parse();
    match dispatch(cli) {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("kartei: {err:#}");
            std::process::exit(1);
        }
    }
}

/// Dispatch one parsed invocation.
///
/// @param cli the parsed command line
/// @return the process exit code
fn dispatch(cli: Cli) -> Result<i32> {
    let db_path = match &cli.db {
        Some(path) => path.clone(),
        None => config::default_db_path()?,
    };

    match cli.command {
        Command::Index { full } => {
            let config = config::Config::load(cli.config.as_deref())?;
            let Some(_lock) = indexer::try_lock(&db_path)? else {
                eprintln!("kartei: another indexer is running, skipping");
                return Ok(0);
            };
            let mut conn = db::open(&db_path)?;
            let stats = indexer::run(&mut conn, &config, full)?;
            eprintln!(
                "kartei: {} repos ({} skipped), {} files indexed, \
                 {} removed, {} symbols, {}ms",
                stats.repos,
                stats.repos_skipped,
                stats.files_indexed,
                stats.files_removed,
                stats.symbols,
                stats.duration_ms
            );
            Ok(0)
        }
        Command::Query {
            query: words,
            limit,
        } => {
            let config = config::Config::load(cli.config.as_deref())?;
            let conn = db::open(&db_path)?;
            let parsed =
                query::parse(&conn, &config.abbreviations, &words.join(" "))?;
            let mut rows = query::run(&conn, &parsed, limit)?;
            // A scrolling terminal leaves the tail in view, so the
            // plumbing prints the best hit last, next to the shell
            // prompt (the pick feed keeps best-first for fzf)
            rows.reverse();
            let mut stdout = std::io::stdout().lock();
            use std::io::Write;
            for row in rows {
                // A broken pipe (eg. `| head`) is a normal way to stop
                if writeln!(stdout, "{}", row.to_tsv()).is_err() {
                    break;
                }
            }
            Ok(0)
        }
        Command::Pick { query: words } => {
            let config = config::Config::load(cli.config.as_deref())?;
            let conn = db::open(&db_path)?;
            pick::run(
                &conn,
                &config,
                cli.config.as_deref(),
                &db_path,
                &words.join(" "),
            )
        }
        Command::Stats {
            repo,
            files,
            symbols,
        } => {
            let conn = db::open(&db_path)?;
            let text = match repo {
                Some(token) => {
                    // Only the repo token needs the config (for the
                    // abbreviation map); global stats stay config-free
                    let config = config::Config::load(cli.config.as_deref())?;
                    let (_, ids) = query::resolve_repos(
                        &conn,
                        &config.abbreviations,
                        &token,
                    )?;
                    // A kind filter alone implies the file listing
                    if files || !symbols.is_empty() {
                        stats::files(&conn, &ids, &symbols)?
                    } else {
                        stats::detail(&conn, &ids)?
                    }
                }
                None => stats::global(&conn)?,
            };
            stats::print(&text);
            Ok(0)
        }
        Command::Init { force } => {
            let path = match &cli.config {
                Some(path) => path.clone(),
                None => config::default_config_path()?,
            };
            if path.exists() && !force {
                anyhow::bail!(
                    "{} exists already (use --force to overwrite)",
                    path.display()
                );
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, config::EXAMPLE_CONFIG)?;
            eprintln!("kartei: wrote {}", path.display());
            Ok(0)
        }
        Command::Candidates { query } => {
            let config = config::Config::load(cli.config.as_deref())?;
            let conn = db::open(&db_path)?;
            pick::candidates(&conn, &config, query.as_deref().unwrap_or(""))?;
            Ok(0)
        }
        Command::Preview { repo, path, line } => {
            let config = config::Config::load(cli.config.as_deref())?;
            let conn = db::open(&db_path)?;
            // File rows pass an empty line column through fzf
            let line = line.and_then(|value| value.parse().ok());
            pick::preview(&conn, &config, &repo, &path, line)?;
            Ok(0)
        }
    }
}
