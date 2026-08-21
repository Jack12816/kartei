//! End-to-end integration tests.
//!
//! These tests drive the real kartei binary (via CARGO_BIN_EXE_kartei)
//! against the examples/ tree in the repository root — the multi-repo
//! testing ground, materialized as git repositories in a temporary
//! copy — and against synthetic throwaway git repositories for the
//! mutation scenarios (touch, delete) that must not modify the
//! checked-in examples.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

/// The examples/ tree beside the crate (the multi-repo testing ground).
fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples")
}

/// The shared, once-indexed examples environment.
///
/// Indexing the examples tree is the expensive part of these tests,
/// so all read-only assertions share a single indexed database; only
/// the mutation scenarios build their own throwaway repositories.
fn examples_env() -> &'static Env {
    static ENV: OnceLock<Env> = OnceLock::new();
    ENV.get_or_init(|| {
        let env = Env::new(&[]);
        let tree = env.dir.path().join("examples");
        materialize_examples(&examples_dir(), &tree);
        env.configure(&[&tree], "");
        env.stderr(&["index"]);
        env
    })
}

/// Copy the examples tree and turn every `.kartei-repo` marked
/// directory into a committed git repository.
///
/// Git cannot track nested repositories, so the checked-in tree marks
/// its repository roots with empty `.kartei-repo` files instead. The
/// markers are dropped from the copy, and the repositories are
/// initialized deepest first, so a parent never records its nested
/// checkouts as tracked content.
///
/// @param source the checked-in examples tree
/// @param target the temporary copy to materialize
fn materialize_examples(source: &Path, target: &Path) {
    copy_tree(source, target);
    let mut roots = Vec::new();
    collect_marked(target, &mut roots);
    roots.sort_by_key(|root| std::cmp::Reverse(root.components().count()));
    for root in roots {
        std::fs::remove_file(root.join(".kartei-repo")).unwrap();
        commit_all(&root);
    }
}

/// Copy a directory tree recursively.
///
/// @param source the directory to copy
/// @param target the destination directory
fn copy_tree(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let dest = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// Collect all directories carrying a `.kartei-repo` marker.
///
/// @param dir the directory to search recursively
/// @param roots the collected repository roots
fn collect_marked(dir: &Path, roots: &mut Vec<PathBuf>) {
    if dir.join(".kartei-repo").is_file() {
        roots.push(dir.to_path_buf());
    }
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_marked(&path, roots);
        }
    }
}

/// Initialize a git repository at the given root and commit all of
/// its content.
///
/// @param root the repository root
fn commit_all(root: &Path) {
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["-c", "user.name=test", "-c", "user.email=test@example.org"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "Initial commit."]);
}

/// A throwaway kartei environment: config, database and start path.
struct Env {
    /// The backing temp directory (deleted on drop).
    dir: tempfile::TempDir,
}

impl Env {
    /// Create an environment whose config points at the given paths.
    ///
    /// @param paths the start paths to configure
    /// @return the ready environment
    fn new(paths: &[&Path]) -> Self {
        Self::with_index_extra(paths, "")
    }

    /// Create an environment with extra `[index]` settings.
    ///
    /// @param paths the start paths to configure
    /// @param extra additional TOML lines for the `[index]` table
    /// @return the ready environment
    fn with_index_extra(paths: &[&Path], extra: &str) -> Self {
        let env = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        env.configure(paths, extra);
        env
    }

    /// Write the configuration pointing at the given start paths.
    ///
    /// @param paths the start paths to configure
    /// @param extra additional TOML lines for the `[index]` table
    fn configure(&self, paths: &[&Path], extra: &str) {
        let list = paths
            .iter()
            .map(|path| format!("{:?}", path.display().to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            self.dir.path().join("config.toml"),
            format!(
                "[index]\npaths = [{list}]\n{extra}\
                 [abbreviations]\naa = \"acme-api\"\n"
            ),
        )
        .unwrap();
    }

    /// Run a kartei subcommand inside this environment.
    ///
    /// @param args the CLI arguments
    /// @return the finished process output
    fn kartei(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kartei"))
            .arg("--config")
            .arg(self.dir.path().join("config.toml"))
            .arg("--db")
            .arg(self.dir.path().join("index.db"))
            .args(args)
            .output()
            .unwrap()
    }

    /// Run a kartei subcommand and return its stdout.
    ///
    /// @param args the CLI arguments
    /// @return the standard output as string
    fn stdout(&self, args: &[&str]) -> String {
        let output = self.kartei(args);
        assert!(
            output.status.success(),
            "kartei {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Run a kartei subcommand and return its stderr.
    ///
    /// @param args the CLI arguments
    /// @return the standard error as string
    fn stderr(&self, args: &[&str]) -> String {
        let output = self.kartei(args);
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stderr).into_owned()
    }
}

/// Create a synthetic git repository with the given files.
///
/// @param root the repository root to create
/// @param files the (path, content) pairs to commit
fn synthetic_repo(root: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(root).unwrap();
    for (path, content) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }
    commit_all(root);
}

#[test]
fn indexes_the_examples_tree() {
    let env = examples_env();
    let repos = env.stdout(&["stats"]);

    for expected in [
        "acme-api",
        "blog-engine",
        "metrics_helper",
        "chat-unread",
        "docker-postgres",
    ] {
        assert!(repos.contains(expected), "missing repo {expected}");
    }
}

#[test]
fn ignores_nested_repositories() {
    let env = examples_env();
    let repos = env.stdout(&["stats"]);

    // The throwaway checkouts below chat-unread/tmp/ must stay
    // invisible
    for nested in ["checkout", "jsonx", "utilx"] {
        assert!(
            !repos.lines().any(|line| line.starts_with(nested)),
            "nested repo {nested} must not be indexed"
        );
    }
}

#[test]
fn second_run_is_a_noop() {
    let env = examples_env();
    let stats = env.stderr(&["index"]);
    assert!(
        stats.contains("0 files indexed, 0 removed"),
        "expected a no-op second run, got: {stats}"
    );
}

#[test]
fn emits_the_documented_tsv_contract() {
    let env = examples_env();
    let output = env.stdout(&["query", "--limit", "50"]);

    assert!(!output.is_empty());
    for line in output.lines() {
        assert_eq!(
            line.split('\t').count(),
            8,
            "TSV row must have 8 columns: {line}"
        );
    }
}

#[test]
fn filters_by_repo_atom_and_abbreviation() {
    let env = examples_env();

    let output = env.stdout(&["query", "aa:"]);
    assert!(!output.is_empty());
    assert!(
        output.lines().all(|line| line.starts_with("acme-api\t")),
        "abbreviation filter must only yield acme-api rows"
    );
}

#[test]
fn rejects_unknown_repo_atoms() {
    let env = examples_env();
    let output = env.kartei(&["query", "nope: foo"]);
    assert!(!output.status.success());
}

#[test]
fn finds_files_by_normalized_paths() {
    let env = examples_env();

    // The file row carries the whole normalized path, so a
    // snake_case term matches regardless of the original casing
    let output = env.stdout(&["query", "application_controller"]);
    assert!(
        output
            .lines()
            .any(|line| { line.contains("application_controller.rb") }),
        "expected application_controller.rb hits, got: {output}"
    );
}

/// Assert that a query yields a row containing all given fragments.
///
/// @param query the kartei query string
/// @param fragments the substrings expected within one TSV row
fn assert_hit(query: &str, fragments: &[&str]) {
    let output = examples_env().stdout(&["query", query]);
    assert!(
        output
            .lines()
            .any(|line| fragments.iter().all(|f| line.contains(f))),
        "no row with {fragments:?} for query {query:?}, got:\n{output}"
    );
}

#[test]
fn finds_real_ruby_classes_with_scope_and_line() {
    // app/api/users_api/api_v1.rb:7 in acme-api
    assert_hit(
        "aa: ApiV1",
        &["app/api/users_api/api_v1.rb", "\t7\t", "class", "UsersApi"],
    );
}

#[test]
fn finds_real_ruby_singleton_methods() {
    assert_hit(
        "metrics_helper: configure",
        &["lib/metrics/helper.rb", "smethod", "configure"],
    );
}

#[test]
fn finds_real_dockerfile_env_variables() {
    assert_hit(
        "docker-redis: DEBIAN_FRONTEND",
        &["7.2/Dockerfile", "env", "DEBIAN_FRONTEND"],
    );
}

#[test]
fn indexes_no_yaml_merge_keys() {
    let env = examples_env();
    // The examples tree carries `<<: *defaults` inheritance wiring
    // (eg. acme-api's docker-compose.yml) — none of it may
    // surface as a key symbol
    let out = env.stdout(&["query", "aa: @key"]);
    assert!(!out.is_empty(), "expected key rows");
    assert!(
        !out.contains("<<"),
        "merge keys must not be indexed, got: {out}"
    );
}

#[test]
fn disables_yaml_resolution_via_config() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("sandbox");
    synthetic_repo(
        &repo,
        &[(
            "config/app.yml",
            "default: &default\n  pool: 5\nproduction:\n  <<: *default\n",
        )],
    );
    let env = Env::with_index_extra(&[dir.path()], "resolve_yaml = false\n");
    env.stderr(&["index"]);
    let out = env.stdout(&["query", "@key pool"]);
    assert!(
        out.contains("default.pool"),
        "spelled-out keys must stay indexed, got: {out}"
    );
    assert!(
        !out.contains("production.pool"),
        "resolve_yaml = false must skip inherited keys, got: {out}"
    );
}

#[test]
fn resolves_inherited_yaml_keys() {
    // acme-api's config/jwt.yml only spells the aud list under
    // `default: &default` — the merge into `production:` must index
    // it under the inheriting path, pointing at the origin line
    assert_hit(
        "aa: @key production.default.aud",
        &["config/jwt.yml", "\t3\t", "production.default.aud"],
    );
}

#[test]
fn finds_real_deep_yaml_keys() {
    assert_hit(
        "pipeline: valkey condition",
        &["docker-compose.yml", "key", "x-common.depends_on.valkey"],
    );
}

#[test]
fn finds_real_erlang_functions() {
    assert_hit(
        "chat-unread: start",
        &["src/mod_unread.erl", "\t22\t", "func", "start"],
    );
}

#[test]
fn finds_real_make_targets() {
    assert_hit(
        "pipeline: start-foreground",
        &["Makefile", "\t91\t", "target", "start-foreground"],
    );
}

#[test]
fn matches_file_paths_across_component_boundaries() {
    // The file row carries the whole normalized path, so a fragment
    // spanning several components matches contiguously (no symbol
    // covers `find_spec` by name, so the file row survives)
    assert_hit(
        "aa: users_api/api_v1 find_spec",
        &["spec/api/users_api/api_v1/find_spec.rb", "\tfile\t"],
    );
}

#[test]
fn matches_scoped_symbols_by_qualified_name() {
    // The scope is part of the qualified matching form: the `::`
    // spelling plus leaf name finds the symbol, not just the file
    assert_hit(
        "aa: users_api::api_v1 find",
        &["\tclass\t", "UsersApi::ApiV1", "Find"],
    );
}

#[test]
fn treats_slash_words_as_path_only_filters() {
    let env = examples_env();
    // `spec/` must not drown in symbols merely named spec — every
    // row lives under a spec path, and the file rows stay listed
    // (a path search is a file search)
    let out = env.stdout(&["query", "aa: spec/"]);
    assert!(!out.is_empty(), "expected rows under spec/");
    for line in out.lines() {
        let path = line.split('\t').nth(1).unwrap_or_default();
        assert!(
            path.contains("spec"),
            "slash words must filter the path only, got: {line}"
        );
    }
    assert!(
        out.lines().any(|line| line.contains("\tfile\t")),
        "pure path-word searches must keep file rows: {out}"
    );
}

#[test]
fn mixes_path_and_symbol_words() {
    // `lib/remote` only occurs in the path, `EntityRef` only in
    // the class name — together they find the class
    assert_hit(
        "aa: lib/remote EntityRef",
        &[
            "entity_reference_v2.rb",
            "\t7\t",
            "class",
            "EntityReferenceV2",
        ],
    );
}

#[test]
fn previews_file_rows_with_an_empty_line_column() {
    let env = examples_env();
    // fzf always passes the line placeholder, empty for file rows —
    // this must render the file, not an argument parse error
    let out = env.stdout(&["preview", "acme-api", "README.md", ""]);
    assert!(!out.is_empty(), "expected preview content");
}

#[test]
fn prints_the_best_hit_last() {
    let env = examples_env();
    // The exact class match is the best hit and ends the stream,
    // right next to the shell prompt
    let out = env.stdout(&["query", "aa: EntityReferenceV2"]);
    let last = out.lines().last().unwrap_or_default();
    assert!(
        last.contains("\tclass\t") && last.contains("\t7\t"),
        "expected the class row last, got: {last}"
    );
}

#[test]
fn drops_file_rows_when_symbols_match_specifically() {
    let env = examples_env();
    let out = env.stdout(&["query", "aa: EntityReferenceV2"]);
    assert!(
        out.lines().any(|line| line.contains("\tclass\t")),
        "expected class hits, got: {out}"
    );
    // The class matches by name, so the path-echoing file rows
    // (entity_reference_v2.rb etc.) must disappear
    assert!(
        !out.lines().any(|line| line.contains("\tfile\t")),
        "file rows must vanish behind specific symbol hits: {out}"
    );
}

#[test]
fn matches_generic_path_fragments() {
    // Generic components like `spec` are first-class path parts
    assert_hit("aa: spec/support", &["spec/support/", "\tfile\t"]);
}

#[test]
fn filters_queries_by_kind_sigil() {
    // The same class row as above, but reached via the @kind filter
    assert_hit(
        "aa: @class ApiV1",
        &["app/api/users_api/api_v1.rb", "\t7\t", "class", "UsersApi"],
    );
}

#[test]
fn resolves_kind_sigil_prefixes_in_queries() {
    assert_hit(
        "metrics_helper: @sme configure",
        &["lib/metrics/helper.rb", "smethod", "configure"],
    );
}

#[test]
fn rejects_unknown_kind_sigils() {
    let env = examples_env();
    let output = env.kartei(&["query", "@nope foo"]);
    assert!(!output.status.success());
}

#[test]
fn lists_landing_files_for_empty_candidates() {
    let env = examples_env();
    let out = env.stdout(&["candidates"]);
    assert!(!out.is_empty(), "landing view must not be empty");
    for line in out.lines() {
        assert!(
            line.contains("@file\tREADME.md"),
            "landing rows must be README.md file rows, got: {line}"
        );
    }
}

#[test]
fn feeds_filtered_candidates_per_query_state() {
    let env = examples_env();
    let out = env.stdout(&["candidates", "aa: @class"]);
    assert!(!out.is_empty());
    for line in out.lines() {
        assert!(
            line.starts_with("acme-api\t")
                && line.contains("\tacme-api@class\t"),
            "candidates must respect repo and kind filters, got: {line}"
        );
    }
}

#[test]
fn paints_matched_term_parts_in_the_candidate_feed() {
    let env = examples_env();
    let out = env.stdout(&["candidates", "aa: user"]);
    assert!(!out.is_empty());
    assert!(
        out.contains("\x1b[31m"),
        "candidate rows must paint the matched term parts, got: {out}"
    );
}

#[test]
fn feeds_nothing_for_unparseable_candidates() {
    let env = examples_env();
    let out = env.stdout(&["candidates", "@zzz"]);
    assert!(out.is_empty(), "half-typed sigils must yield no rows");
}

#[test]
fn combines_multiple_query_words_as_and() {
    // Both words must hit the same row, regardless of their casing —
    // the qualified class name covers both, so it is the specific
    // hit that survives (its file row is dropped behind it)
    assert_hit(
        "aa: UsersApi find",
        &["app/api/users_api/api_v1/find.rb", "\tclass\t", "Find"],
    );
}

#[test]
fn reindexes_only_touched_files() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("sandbox");
    synthetic_repo(
        &repo,
        &[
            ("a.rb", "class Alpha\nend\n"),
            ("b.rb", "class Beta\nend\n"),
        ],
    );
    let env = Env::new(&[dir.path()]);
    env.stderr(&["index"]);

    // Rewrite one file with a new mtime; only that file may re-index
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(repo.join("a.rb"), "class Gamma\nend\n").unwrap();
    let stats = env.stderr(&["index"]);
    assert!(
        stats.contains("1 files indexed"),
        "expected exactly one re-indexed file, got: {stats}"
    );
}

#[test]
fn removes_rows_of_deleted_files() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("sandbox");
    synthetic_repo(
        &repo,
        &[
            ("a.rb", "class Alpha\nend\n"),
            ("b.rb", "class Beta\nend\n"),
        ],
    );
    let env = Env::new(&[dir.path()]);
    env.stderr(&["index"]);

    std::fs::remove_file(repo.join("b.rb")).unwrap();
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rm", "-q", "--cached", "b.rb"])
        .output()
        .unwrap();
    assert!(output.status.success());

    let stats = env.stderr(&["index"]);
    assert!(
        stats.contains("1 removed"),
        "expected one removed file, got: {stats}"
    );
    let rows = env.stdout(&["query", "beta"]);
    assert!(
        rows.is_empty(),
        "symbols of deleted files must vanish, got: {rows}"
    );
}

#[test]
fn full_reindex_rebuilds_everything() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("sandbox");
    synthetic_repo(&repo, &[("a.rb", "class Alpha\nend\n")]);
    let env = Env::new(&[dir.path()]);
    env.stderr(&["index"]);
    let stats = env.stderr(&["index", "--full"]);
    assert!(
        stats.contains("1 files indexed"),
        "expected a full re-extraction, got: {stats}"
    );
}

#[test]
fn skips_configured_ignored_path_parts() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("sandbox");
    synthetic_repo(
        &repo,
        &[
            ("a.rb", "class Alpha\nend\n"),
            (
                "spec/fixtures/vcr_cassettes/cassette.yml",
                "cassette: recorded\n",
            ),
            ("spec/snapshots/widget.snap", "snapshot: recorded\n"),
            ("public/application.min.js", "var minified = 1\n"),
        ],
    );
    let env = Env::with_index_extra(
        &[dir.path()],
        "ignores = [\"/fixtures/vcr_cassettes/\", \"glob:**/*.snap\", \
         'regex:\\.min\\.js$']\n",
    );
    let stats = env.stderr(&["index"]);
    assert!(
        stats.contains("1 files indexed"),
        "only a.rb may be indexed, got: {stats}"
    );

    // Neither file rows nor symbols may exist for the ignored paths
    for term in ["cassette", "snap", "minified", "application"] {
        let rows = env.stdout(&["query", term]);
        assert!(
            rows.is_empty(),
            "ignored paths must yield no rows for {term}, got: {rows}"
        );
    }
}

#[test]
fn skips_extensionless_binaries() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("sandbox");
    synthetic_repo(
        &repo,
        &[
            (
                "exe/tool",
                "#!/usr/bin/env bash\nfunction hi()\n{\n  :\n}\n",
            ),
            ("blob", "\u{0}\u{1}binary"),
        ],
    );
    let env = Env::new(&[dir.path()]);
    env.stderr(&["index"]);

    // The shebang script is parsed as bash, the NUL blob is skipped
    // but still present as file row
    let rows = env.stdout(&["query", "blob"]);
    assert!(rows.lines().any(|line| line.contains("\tfile\t")));
}
