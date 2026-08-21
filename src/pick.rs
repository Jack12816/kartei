//! The fzf-backed pick porcelain.
//!
//! `kartei pick` runs fzf as a pure display: fzf's own matcher is
//! disabled and every keystroke reloads the candidate list through the
//! hidden `candidates` subcommand, so the query grammar
//! (`[repo:] [@kind ...] [term words ...]`) is evaluated against the
//! database live. An empty input shows the landing view — one
//! configured landing file per repository. The selection is printed as
//! `<path>[:LINE]` with an absolute path — or `<key>\t<path>[:LINE]`
//! when a configured --expect key confirmed the selection. Launching
//! editors or terminals stays in the caller's shell wrapper; see the
//! README for a ready-made example.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use rusqlite::Connection;

use crate::config::Config;
use crate::query;

/// The exit code signalling an aborted pick (mirrors fzf).
pub const ABORT_CODE: i32 = 130;

/// Run the interactive pick and print the selection.
///
/// @param conn the open database connection
/// @param config the loaded configuration
/// @param config_path the config file override, or +nil+ for the
///   default location
/// @param db_path the database file path
/// @param input the raw query string preseeding the prompt
/// @return the process exit code
pub fn run(
    conn: &Connection,
    config: &Config,
    config_path: Option<&Path>,
    db_path: &Path,
    input: &str,
) -> Result<i32> {
    // Validate the initial input up front, so a broken atom fails in
    // the terminal instead of silently emptying the fzf list
    query::parse(conn, &config.abbreviations, input)?;

    let fzf = spawn_fzf(config, config_path, db_path, input)?;
    let output = fzf.wait_with_output().context("failed to run fzf")?;
    if !output.status.success() && output.stdout.is_empty() {
        return Ok(ABORT_CODE);
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let mut lines = raw.lines();
    // With --expect the first output line is the pressed key (empty
    // for plain enter), the second the selected row
    let key = if config.pick.expect.is_empty() {
        ""
    } else {
        lines.next().unwrap_or("")
    };
    let Some(selection) = lines.next() else {
        return Ok(ABORT_CODE);
    };

    let Some(target) = selection_target(conn, selection) else {
        bail!("unexpected fzf selection: {selection}");
    };
    match key.is_empty() {
        true => println!("{target}"),
        false => println!("{key}\t{target}"),
    }
    Ok(0)
}

/// Emit the pick feed candidates for one query state.
///
/// This is the hidden `candidates` subcommand fzf reloads on every
/// keystroke: an empty input renders the landing view, everything
/// else runs the regular query capped at the configured pick limit.
/// Unparseable input (eg. a half-typed sigil) yields an empty list
/// instead of an error — the UI recovers with the next keystroke.
///
/// @param conn the open database connection
/// @param config the loaded configuration
/// @param input the current query string
pub fn candidates(
    conn: &Connection,
    config: &Config,
    input: &str,
) -> Result<()> {
    let (rows, words) = if input.trim().is_empty() {
        (query::landing(conn, &config.pick.landing)?, Vec::new())
    } else {
        match query::parse(conn, &config.abbreviations, input) {
            Ok(parsed) => (
                query::run(conn, &parsed, Some(config.pick.limit))?,
                parsed.words(),
            ),
            Err(_) => (Vec::new(), Vec::new()),
        }
    };

    let mut stdout = std::io::stdout().lock();
    for row in rows {
        let line = row.to_pick(&words, &config.pick.match_highlight);
        // A broken pipe just means fzf reloaded again already
        if writeln!(stdout, "{line}").is_err() {
            break;
        }
    }
    Ok(())
}

/// Spawn fzf as a pure display for the reload-driven candidate feed.
///
/// @param config the loaded configuration
/// @param config_path the config file override, or +nil+ for the
///   default location
/// @param db_path the database file path
/// @param input the initial fzf query
/// @return the running fzf child process
fn spawn_fzf(
    config: &Config,
    config_path: Option<&Path>,
    db_path: &Path,
    input: &str,
) -> Result<std::process::Child> {
    let feed =
        format!("{} candidates {{q}}", self_command(config_path, db_path));
    let mut command = Command::new("fzf");
    command
        .args(["--delimiter", "\t"])
        // Show repo@kind, scoped name and path:LINE; the machine
        // columns repo/path/line stay hidden
        .args(["--with-nth", "4,5,6"])
        // kartei owns all matching — fzf only displays and reloads
        .arg("--disabled")
        // The feed paints the matched term parts itself (fzf's own
        // matcher is disabled and cannot); fzf strips the codes from
        // placeholders and the selection output, so the machine
        // columns stay clean
        .arg("--ansi")
        .args(["--bind", &format!("start:reload:{feed}")])
        .args(["--bind", &format!("change:reload:{feed}")])
        // Snap the pointer back to the best hit whenever a reload
        // lands: the feed is most-relevant-first and its first row
        // renders next to the prompt, so typing always leaves the
        // top candidate selected instead of a stale row offset
        .args(["--bind", "load:first"])
        // Escape clears the current input back to the landing view;
        // only a second escape on the empty input aborts
        .args([
            "--bind",
            "esc:transform:[ -n {q} ] && echo clear-query || echo abort",
        ])
        // Readline line editing: fzf binds ctrl-k to list-up by
        // default, but next to ctrl-u it has to kill to end of line
        .args(["--bind", "ctrl-k:kill-line"])
        .args(["--preview", &preview_command(config_path, db_path)])
        // Scroll the preview to the symbol line (field 3), centered;
        // the preview emits one row per file line, so the field value
        // addresses the row directly (file rows leave it empty, which
        // falls back to the top of the file)
        .args(["--preview-window", "noborder,+{3}-/2"]);
    if !input.is_empty() {
        command.args(["--query", input]);
    }
    if !config.pick.expect.is_empty() {
        command.args(["--expect", &config.pick.expect.join(",")]);
    }
    command.args(&config.pick.fzf_args);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn fzf (is it installed?)")
}

/// Build the self-invocation prefix for fzf-spawned subcommands.
///
/// The current binary is re-invoked with the same --config/--db
/// flags, so reload and preview always operate on the databases the
/// pick itself runs on.
///
/// @param config_path the config file override, or +nil+ for the
///   default location
/// @param db_path the database file path
/// @return the shell command prefix
fn self_command(config_path: Option<&Path>, db_path: &Path) -> String {
    let exe = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "kartei".to_string());
    let mut command = shell_quote(&exe);
    if let Some(path) = config_path {
        command.push_str(" --config ");
        command.push_str(&shell_quote(&path.display().to_string()));
    }
    command.push_str(" --db ");
    command.push_str(&shell_quote(&db_path.display().to_string()));
    command
}

/// Quote a string for safe use in a POSIX shell command line.
///
/// @param raw the raw string
/// @return the single-quoted shell word
fn shell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\\''"))
}

/// Build the fzf preview command line.
///
/// The current kartei binary serves as its own preview renderer via
/// the hidden `preview` subcommand, so no external pager is required.
///
/// @param config_path the config file override, or +nil+ for the
///   default location
/// @param db_path the database file path
/// @return the preview shell command
fn preview_command(config_path: Option<&Path>, db_path: &Path) -> String {
    format!(
        "{} preview {{1}} {{2}} {{3}}",
        self_command(config_path, db_path)
    )
}

/// Turn a selected feed row into the printable target.
///
/// @param conn the open database connection
/// @param selection the selected feed row
/// @return `<absolute-path>[:LINE]`, or +nil+ for malformed rows
fn selection_target(conn: &Connection, selection: &str) -> Option<String> {
    let fields: Vec<&str> = selection.split('\t').collect();
    let (repo, path, line) = (fields.first()?, fields.get(1)?, fields.get(2)?);
    let root: String = conn
        .query_row("SELECT root FROM repos WHERE name = ?1", [*repo], |row| {
            row.get(0)
        })
        .ok()?;
    let absolute = format!("{root}/{path}");
    match line.is_empty() {
        true => Some(absolute),
        false => Some(format!("{absolute}:{line}")),
    }
}

/// Render the preview window for one candidate row.
///
/// The whole file runs through the configured highlighter (or the
/// built-in plain renderer as fallback) and the symbol line is
/// painted with the configured ANSI background. Every output row
/// maps 1:1 to a file line, so the fzf preview window scrolls
/// straight to the symbol line via its `+{3}` offset expression;
/// used internally by the fzf invocation of `kartei pick`.
///
/// @param conn the open database connection
/// @param config the loaded configuration
/// @param repo the repository name
/// @param path the repo-relative file path
/// @param line the 1-based symbol line, or +nil+ for file rows
pub fn preview(
    conn: &Connection,
    config: &Config,
    repo: &str,
    path: &str,
    line: Option<u32>,
) -> Result<()> {
    let root: String = conn
        .query_row("SELECT root FROM repos WHERE name = ?1", [repo], |row| {
            row.get(0)
        })
        .with_context(|| format!("unknown repository {repo}"))?;
    let absolute = format!("{root}/{path}");

    let lines = highlighted_lines(config, &absolute)
        .unwrap_or_else(|| plain_lines(&absolute, line));

    let mut stdout = std::io::stdout().lock();
    for (idx, text) in lines.iter().enumerate() {
        let text = match Some((idx + 1) as u32) == line {
            true => paint_line(text, &config.pick.highlight),
            false => text.clone(),
        };
        // A broken pipe just means fzf moved on to the next preview
        if writeln!(stdout, "{text}").is_err() {
            break;
        }
    }
    Ok(())
}

/// Run the configured highlighter over a file.
///
/// The file path replaces a `{file}` placeholder in the command, or
/// is appended when none is given; the command runs through `sh -c`.
/// The command must emit exactly one output line per input line, so
/// the symbol-line painting and the fzf preview scroll can address
/// their line by number.
///
/// @param config the loaded configuration
/// @param path the absolute file path
/// @return the highlighted lines, or +nil+ when no highlighter is
///   configured or the command fails
fn highlighted_lines(config: &Config, path: &str) -> Option<Vec<String>> {
    let template = config.pick.highlighter.trim();
    if template.is_empty() {
        return None;
    }
    let command = match template.contains("{file}") {
        true => template.replace("{file}", &shell_quote(path)),
        false => format!("{template} {}", shell_quote(path)),
    };
    let output = Command::new("sh").args(["-c", &command]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(String::from)
            .collect(),
    )
}

/// Render a file with the built-in plain renderer.
///
/// Numbers every line and marks the symbol line with a `>` caret —
/// the fallback whenever no highlighter is configured or available.
///
/// @param path the absolute file path
/// @param line the 1-based symbol line, or +nil+ for file rows
/// @return the rendered lines
fn plain_lines(path: &str, line: Option<u32>) -> Vec<String> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    content
        .lines()
        .enumerate()
        .map(|(idx, text)| {
            let number = idx + 1;
            let marker = match Some(number as u32) == line {
                true => ">",
                false => " ",
            };
            format!("{marker}{number:5} {text}")
        })
        .collect()
}

/// Paint one rendered line with the configured ANSI background.
///
/// The background is re-armed after every SGR reset, so color resets
/// emitted by the highlighter within the line cannot switch the
/// marker off halfway through.
///
/// @param text the rendered line
/// @param sgr the configured SGR parameters (eg. `48;5;238`)
/// @return the painted line
fn paint_line(text: &str, sgr: &str) -> String {
    if sgr.trim().is_empty() {
        return text.to_string();
    }
    let arm = format!("\x1b[{sgr}m");
    let rearmed = text.replace("\x1b[0m", &format!("\x1b[0m{arm}"));
    format!("{arm}{rearmed}\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open an in-memory database with one known repository.
    fn conn_with_repo() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE repos (id INTEGER PRIMARY KEY, name TEXT,
               root TEXT, head TEXT, dirty INTEGER, indexed_at INTEGER);
             INSERT INTO repos VALUES (1, 'acme-api', '/r/acme-api', NULL, 0, 0);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn builds_targets_with_line() {
        let target = selection_target(
            &conn_with_repo(),
            "acme-api\tapp/models/user.rb\t3\t[class]\tUser\tapp/models/user.rb:3",
        );
        assert_eq!(target.as_deref(), Some("/r/acme-api/app/models/user.rb:3"));
    }

    #[test]
    fn builds_targets_without_line() {
        let target = selection_target(
            &conn_with_repo(),
            "acme-api\tREADME.md\t\t[file]\tREADME.md\tREADME.md",
        );
        assert_eq!(target.as_deref(), Some("/r/acme-api/README.md"));
    }

    #[test]
    fn rejects_unknown_repos() {
        assert_eq!(selection_target(&conn_with_repo(), "x\ta\t1"), None);
    }

    #[test]
    fn quotes_shell_words_with_single_quotes() {
        assert_eq!(shell_quote("/tmp/it's"), "'/tmp/it'\\''s'");
    }

    #[test]
    fn paints_lines_with_the_configured_background() {
        assert_eq!(
            paint_line("plain", "48;5;238"),
            "\x1b[48;5;238mplain\x1b[0m"
        );
    }

    #[test]
    fn rearms_the_background_after_color_resets() {
        assert_eq!(
            paint_line("a\x1b[0mb", "41"),
            "\x1b[41ma\x1b[0m\x1b[41mb\x1b[0m"
        );
    }

    #[test]
    fn keeps_lines_untouched_without_a_highlight() {
        assert_eq!(paint_line("plain", ""), "plain");
    }
}
