//! Configuration loading and file-system locations.
//!
//! The configuration lives in TOML form at
//! `$XDG_CONFIG_HOME/kartei/config.toml`, the database at
//! `$XDG_DATA_HOME/kartei/index.db`; both are overridable via CLI
//! flags. A missing configuration file is a hard error pointing to
//! `kartei init`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use etcetera::BaseStrategy;
use serde::Deserialize;

/// The default maximum file size considered for symbol extraction.
const DEFAULT_MAX_FILE_SIZE: u64 = 2 * 1024 * 1024;

/// The default keys fzf reports via --expect in `kartei pick`.
const DEFAULT_EXPECT: &[&str] = &["btab", "ctrl-space", "tab"];

/// The default landing file shown per repository on empty pick input.
const DEFAULT_LANDING: &str = "README.md";

/// The default maximum number of pick candidates per keystroke.
const DEFAULT_PICK_LIMIT: usize = 100;

/// The default preview highlighter command.
const DEFAULT_HIGHLIGHTER: &str = "bat --color=always --style=numbers";

/// The default ANSI SGR parameters painting the previewed line.
const DEFAULT_HIGHLIGHT: &str = "48;5;238";

/// The default ANSI SGR parameters painting matched term parts.
const DEFAULT_MATCH_HIGHLIGHT: &str = "31";

/// The example configuration written by `kartei init`.
pub const EXAMPLE_CONFIG: &str = r#"[index]
# Start paths; each is recursively searched for git repository roots.
paths = ["/home/you/projects"]

# Nested repositories: the walk stops at the first repository root, so
# repositories within repositories (eg. throwaway clones below tmp/)
# stay invisible. Directories listed here are the exception — below
# them every nested repository, at any depth, is discovered and
# indexed as a repository of its own, named after the enclosing
# repository plus its path within (`kartei/kartei`, `app/tmp/vendor`).
# nested = ["/home/you/projects/platform-monorepo"]

# Ignore list: matching files are skipped entirely at indexing (no
# file or symbol rows). Every entry matches against the repo-relative
# file path in one of three flavors:
#   * path part (default): one or more complete path components
#     matched anywhere in the path ("/fixtures/vcr_cassettes/")
#   * glob: a file glob; `*` stays within one path component, `**`
#     crosses components ("glob:**/*.snap")
#   * regex: an unanchored regular expression ('regex:\.min\.js$')
# A change applies to a repository on its next re-index
# (`kartei index --full` for unchanged repos).
# ignores = ["/fixtures/vcr_cassettes/", "glob:**/*.snap"]

# Files larger than this many bytes are skipped for symbol extraction
# (they still get their file/path row).
# max_file_size = 2097152

# Resolve YAML anchors, aliases and merge keys (`<<: *defaults`)
# into inherited key symbols: a key only written under
# `default: &default` is also indexed as `production...` when
# merged, pointing at its origin line. A change applies on the next
# re-index (`kartei index --full` for unchanged repos).
# resolve_yaml = true

[abbreviations]
# Query-prefix abbreviations: `aa: term` == `acme-api: term`.
# aa = "acme-api"

[pick]
# Extra keys fzf reports via --expect; a selection confirmed by one of
# them is printed as "<key>\t<path>[:LINE]".
# expect = ["btab", "ctrl-space", "tab"]

# The repo-relative file listed once per repository when the pick
# input is empty (the project-browser landing view).
# landing = "README.md"

# Maximum number of candidate rows shown per keystroke.
# limit = 100

# Preview highlighter command. The file path replaces a {file}
# placeholder, or is appended when none is given. The command must
# emit exactly one output line per input line (bat's grid/header
# styles break that), so line painting and preview scrolling can
# address lines by number; an empty string uses the built-in plain
# renderer. Eg. a custom script: highlighter = "~/.bin/code"
# highlighter = "bat --color=always --style=numbers"

# ANSI SGR parameters painting the symbol line in the preview
# (eg. "48;5;238" for a grey background); empty disables it.
# highlight = "48;5;238"

# ANSI SGR parameters painting the matched term parts in the fzf
# candidate list (eg. "31" for dark red, "1;31" for a bold variant);
# empty disables it.
# match_highlight = "31"

# Extra arguments appended to the fzf invocation.
# fzf_args = []
"#;

/// The whole kartei configuration as loaded from config.toml.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Indexer settings.
    pub index: IndexConfig,
    /// Query-prefix abbreviation map (abbreviation -> repo name).
    #[serde(default)]
    pub abbreviations: BTreeMap<String, String>,
    /// Pick porcelain settings.
    #[serde(default)]
    pub pick: PickConfig,
}

/// Indexer settings from the `[index]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexConfig {
    /// Start paths recursively searched for git repository roots.
    pub paths: Vec<PathBuf>,
    /// Directories below which nested repositories (repos within
    /// repos, at any depth) are discovered and indexed too.
    #[serde(default)]
    pub nested: Vec<PathBuf>,
    /// Ignore entries whose files are skipped entirely at indexing.
    #[serde(default)]
    pub ignores: Vec<String>,
    /// Maximum file size in bytes considered for symbol extraction.
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,
    /// Whether Yaml anchors, aliases and merge keys resolve into
    /// inherited key symbols.
    #[serde(default = "default_resolve_yaml")]
    pub resolve_yaml: bool,
}

impl IndexConfig {
    /// Compile the ignore entries into a reusable matcher.
    ///
    /// Entries prefixed `glob:` compile as file globs, `regex:` as
    /// regular expressions; everything else is a path part — one or
    /// more complete path components matched anywhere in the path,
    /// never inside a single component name.
    ///
    /// @return the compiled ignore matcher
    /// @raise when a glob or regex entry is invalid
    pub fn ignore_matcher(&self) -> Result<IgnoreMatcher> {
        let mut parts = Vec::new();
        let mut globs = globset::GlobSetBuilder::new();
        let mut regexes = Vec::new();
        for entry in &self.ignores {
            if let Some(pattern) = entry.strip_prefix("glob:") {
                // Keep `*` within one path component so globs behave
                // path-aware; `**` crosses components explicitly
                let glob = globset::GlobBuilder::new(pattern)
                    .literal_separator(true)
                    .build()
                    .with_context(|| {
                        format!("invalid ignore glob '{pattern}'")
                    })?;
                globs.add(glob);
            } else if let Some(pattern) = entry.strip_prefix("regex:") {
                regexes.push(pattern.to_string());
            } else {
                let part = entry.trim_matches('/');
                if !part.is_empty() {
                    parts.push(format!("/{part}/"));
                }
            }
        }
        Ok(IgnoreMatcher {
            parts,
            globs: globs.build().context("invalid ignore glob")?,
            regexes: regex::RegexSet::new(&regexes)
                .context("invalid ignore regex")?,
        })
    }
}

/// The compiled `index.ignores` list, matching repo-relative file
/// paths against path parts, file globs and regular expressions.
pub struct IgnoreMatcher {
    /// Slash-wrapped path parts for component-exact containment.
    parts: Vec<String>,
    /// The compiled `glob:` entries.
    globs: globset::GlobSet,
    /// The compiled `regex:` entries.
    regexes: regex::RegexSet,
}

impl IgnoreMatcher {
    /// Whether a repo-relative path matches any ignore entry.
    ///
    /// @param path the repo-relative file path
    /// @return whether the path is ignored
    pub fn matches(&self, path: &str) -> bool {
        let wrapped = format!("/{path}/");
        self.parts.iter().any(|part| wrapped.contains(part))
            || self.globs.is_match(path)
            || self.regexes.is_match(path)
    }
}

/// Pick porcelain settings from the `[pick]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickConfig {
    /// Keys fzf reports via --expect.
    #[serde(default = "default_expect")]
    pub expect: Vec<String>,
    /// The repo-relative landing file shown on empty pick input.
    #[serde(default = "default_landing")]
    pub landing: String,
    /// Maximum number of candidate rows shown per keystroke.
    #[serde(default = "default_pick_limit")]
    pub limit: usize,
    /// The preview highlighter command, or empty for the built-in
    /// plain renderer.
    #[serde(default = "default_highlighter")]
    pub highlighter: String,
    /// ANSI SGR parameters painting the previewed symbol line, or
    /// empty to disable the highlight.
    #[serde(default = "default_highlight")]
    pub highlight: String,
    /// ANSI SGR parameters painting the matched term parts in the
    /// candidate list, or empty to disable the highlight.
    #[serde(default = "default_match_highlight")]
    pub match_highlight: String,
    /// Extra arguments appended to the fzf invocation.
    #[serde(default)]
    pub fzf_args: Vec<String>,
}

impl Default for PickConfig {
    /// Build the pick settings with all defaults applied.
    fn default() -> Self {
        Self {
            expect: default_expect(),
            landing: default_landing(),
            limit: default_pick_limit(),
            highlighter: default_highlighter(),
            highlight: default_highlight(),
            match_highlight: default_match_highlight(),
            fzf_args: Vec::new(),
        }
    }
}

/// The default maximum extractable file size.
fn default_max_file_size() -> u64 {
    DEFAULT_MAX_FILE_SIZE
}

/// The default Yaml resolution switch: enabled.
fn default_resolve_yaml() -> bool {
    true
}

/// Build the default --expect key list as owned strings.
fn default_expect() -> Vec<String> {
    DEFAULT_EXPECT.iter().map(ToString::to_string).collect()
}

/// The default landing file as owned string.
fn default_landing() -> String {
    DEFAULT_LANDING.to_string()
}

/// The default pick candidate limit.
fn default_pick_limit() -> usize {
    DEFAULT_PICK_LIMIT
}

/// The default preview highlighter as owned string.
fn default_highlighter() -> String {
    DEFAULT_HIGHLIGHTER.to_string()
}

/// The default preview highlight SGR parameters as owned string.
fn default_highlight() -> String {
    DEFAULT_HIGHLIGHT.to_string()
}

/// The default match highlight SGR parameters as owned string.
fn default_match_highlight() -> String {
    DEFAULT_MATCH_HIGHLIGHT.to_string()
}

impl Config {
    /// Load the configuration from the given or the default location.
    ///
    /// @param path the config file override, or +nil+ for the XDG
    ///   default location
    /// @return the parsed configuration
    /// @raise when the file is missing or invalid
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let path = match path {
            Some(path) => path.to_path_buf(),
            None => default_config_path()?,
        };
        let raw = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "failed to read config {} (create one via `kartei init`)",
                path.display()
            )
        })?;
        let config: Config = toml::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        if config.index.paths.is_empty() {
            bail!("{}: index.paths must not be empty", path.display());
        }
        // Fail fast on broken ignore patterns, no matter which
        // subcommand loaded the configuration
        config
            .index
            .ignore_matcher()
            .with_context(|| format!("failed to load {}", path.display()))?;
        Ok(config)
    }
}

/// The default configuration file location.
///
/// @return the XDG-based config.toml path
pub fn default_config_path() -> Result<PathBuf> {
    let strategy = etcetera::choose_base_strategy()
        .context("failed to determine the XDG base directories")?;
    Ok(strategy.config_dir().join("kartei").join("config.toml"))
}

/// The default database location.
///
/// @return the XDG-based index.db path
pub fn default_db_path() -> Result<PathBuf> {
    let strategy = etcetera::choose_base_strategy()
        .context("failed to determine the XDG base directories")?;
    Ok(strategy.data_dir().join("kartei").join("index.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a config from an inline TOML string.
    fn parse(raw: &str) -> Result<Config> {
        Ok(toml::from_str(raw)?)
    }

    #[test]
    fn parses_minimal_config_with_defaults() {
        let config = parse("[index]\npaths = [\"/tmp\"]").unwrap();
        assert_eq!(config.index.paths, vec![PathBuf::from("/tmp")]);
    }

    #[test]
    fn parses_empty_default_ignores() {
        let config = parse("[index]\npaths = [\"/tmp\"]").unwrap();
        assert!(config.index.ignores.is_empty());
    }

    #[test]
    fn parses_empty_default_nested_roots() {
        let config = parse("[index]\npaths = [\"/tmp\"]").unwrap();
        assert!(config.index.nested.is_empty());
    }

    #[test]
    fn parses_nested_roots() {
        let config =
            parse("[index]\npaths = [\"/tmp\"]\nnested = [\"/tmp/mono\"]")
                .unwrap();
        assert_eq!(config.index.nested, vec![PathBuf::from("/tmp/mono")]);
    }

    /// Build a compiled ignore matcher from the given entries.
    fn matcher(entries: &[&str]) -> Result<IgnoreMatcher> {
        let list = entries
            .iter()
            .map(|entry| format!("{entry:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        parse(&format!("[index]\npaths = [\"/tmp\"]\nignores = [{list}]"))?
            .index
            .ignore_matcher()
    }

    #[test]
    fn ignores_paths_containing_the_part() {
        let matcher = matcher(&["/fixtures/vcr_cassettes/"]).unwrap();
        assert!(matcher.matches("spec/fixtures/vcr_cassettes/find.yml"));
    }

    #[test]
    fn ignores_parts_anchored_at_the_path_start() {
        let matcher = matcher(&["fixtures/vcr_cassettes"]).unwrap();
        assert!(matcher.matches("fixtures/vcr_cassettes/find.yml"));
    }

    #[test]
    fn keeps_paths_matching_only_part_of_a_component() {
        let matcher = matcher(&["fixtures/vcr_cassettes"]).unwrap();
        assert!(!matcher.matches("myfixtures/vcr_cassettes/find.yml"));
    }

    #[test]
    fn keeps_paths_without_the_ignored_part() {
        let matcher = matcher(&["tmp"]).unwrap();
        assert!(!matcher.matches("app/tmpl/find.rb"));
    }

    #[test]
    fn ignores_paths_matching_a_deep_glob() {
        let matcher = matcher(&["glob:**/*.snap"]).unwrap();
        assert!(matcher.matches("spec/snapshots/find.snap"));
    }

    #[test]
    fn keeps_nested_paths_against_a_flat_glob() {
        let matcher = matcher(&["glob:*.snap"]).unwrap();
        assert!(matcher.matches("find.snap"));
        assert!(!matcher.matches("spec/snapshots/find.snap"));
    }

    #[test]
    fn ignores_paths_matching_a_regex() {
        let matcher = matcher(&["regex:\\.min\\.(js|css)$"]).unwrap();
        assert!(matcher.matches("public/assets/application.min.js"));
    }

    #[test]
    fn keeps_paths_not_matching_a_regex() {
        let matcher = matcher(&["regex:\\.min\\.(js|css)$"]).unwrap();
        assert!(!matcher.matches("public/assets/application.js"));
    }

    #[test]
    fn rejects_invalid_ignore_globs() {
        assert!(matcher(&["glob:[oops"]).is_err());
    }

    #[test]
    fn rejects_invalid_ignore_regexes() {
        assert!(matcher(&["regex:(oops"]).is_err());
    }

    #[test]
    fn parses_default_max_file_size() {
        let config = parse("[index]\npaths = [\"/tmp\"]").unwrap();
        assert_eq!(config.index.max_file_size, DEFAULT_MAX_FILE_SIZE);
    }

    #[test]
    fn parses_default_yaml_resolution() {
        let config = parse("[index]\npaths = [\"/tmp\"]").unwrap();
        assert!(config.index.resolve_yaml);
    }

    #[test]
    fn parses_default_expect_keys() {
        let config = parse("[index]\npaths = [\"/tmp\"]").unwrap();
        assert_eq!(config.pick.expect, default_expect());
    }

    #[test]
    fn parses_default_pick_landing() {
        let config = parse("[index]\npaths = [\"/tmp\"]").unwrap();
        assert_eq!(config.pick.landing, DEFAULT_LANDING);
    }

    #[test]
    fn parses_default_pick_limit() {
        let config = parse("[index]\npaths = [\"/tmp\"]").unwrap();
        assert_eq!(config.pick.limit, DEFAULT_PICK_LIMIT);
    }

    #[test]
    fn parses_default_pick_highlighter() {
        let config = parse("[index]\npaths = [\"/tmp\"]").unwrap();
        assert_eq!(config.pick.highlighter, DEFAULT_HIGHLIGHTER);
    }

    #[test]
    fn parses_default_pick_highlight() {
        let config = parse("[index]\npaths = [\"/tmp\"]").unwrap();
        assert_eq!(config.pick.highlight, DEFAULT_HIGHLIGHT);
    }

    #[test]
    fn parses_default_pick_match_highlight() {
        let config = parse("[index]\npaths = [\"/tmp\"]").unwrap();
        assert_eq!(config.pick.match_highlight, DEFAULT_MATCH_HIGHLIGHT);
    }

    #[test]
    fn parses_abbreviations() {
        let config = parse(
            "[index]\npaths = [\"/tmp\"]\n\
             [abbreviations]\naa = \"acme-api\"",
        )
        .unwrap();
        assert_eq!(
            config.abbreviations.get("aa").map(String::as_str),
            Some("acme-api")
        );
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(parse("[index]\npaths = []\ntypo = 1").is_err());
    }

    #[test]
    fn example_config_is_parseable() {
        assert!(parse(EXAMPLE_CONFIG).is_ok());
    }
}
