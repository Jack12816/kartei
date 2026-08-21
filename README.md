<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/project-dark.png">
    <source media="(prefers-color-scheme: light)" srcset="docs/assets/project-light.png">
    <img src="docs/assets/project-light.png" alt="kartei" width="600">
  </picture>
  <br>
  I got 99 repos, <del>but</del> and finding shit <del>ain’t</del> is one.
  <br>
  <br>
</p>

[![Continuous Integration](https://github.com/Jack12816/kartei/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/Jack12816/kartei/actions/workflows/ci.yml)

A personal code-symbol index and search CLI. kartei discovers git
repositories under configured start paths, extracts classes, modules,
methods, constants, make targets, YAML keys, markdown headings and
more with tree-sitter, and keeps everything in one SQLite database
for incremental, timer-friendly refreshes. The query side either
emits stable TSV (plumbing) or drives fzf directly (porcelain) and
prints `path:LINE` targets ready for `vim path/to/file:9`.

The name is German: a Kartei is a card index — the physical data
structure this tool mirrors digitally.

## Why

Grepping file lists gets you to files; kartei gets you to *symbols*
with their exact line, across every repository you work on, matched
case-insensitively across naming conventions: `FooBar`, `foo_bar`,
`foo-bar` and `Foo::Bar` all hit the same index key.

## Demo

[![asciicast](https://asciinema.org/a/UDNBAAgUCAbBKPXt.svg)](https://asciinema.org/a/UDNBAAgUCAbBKPXt)

## Install

See `docs/INSTALL.md` for the full guide (build, config bootstrap,
systemd user timer). Short version:

    cargo install --path .
    kartei init                 # writes ~/.config/kartei/config.toml
    $EDITOR ~/.config/kartei/config.toml
    kartei index                # first full run
    systemctl --user enable --now kartei-index.timer

## Usage

    kartei index [--full]       # incremental (or forced full) indexing
    kartei query [QUERY]        # emit TSV candidates (plumbing)
    kartei pick [QUERY]         # interactive fzf pick (porcelain)
    kartei stats [REPO]         # index statistics, global or per repo
    kartei stats REPO --files   # list indexed files and their symbols
    kartei stats REPO --symbols=KIND,..  # ...only these symbol kinds
    kartei init [--force]       # write an example configuration

`QUERY` is `[repo:] [@kind ...] [term words ...]` — every part
optional:

    kartei query acme-api: user_policy
    kartei query aa: UserPolicy       # abbreviation, same result
    kartei query acme: user           # unique repo-name prefix works
    kartei query aa: @class user      # only classes
    kartei query @target docker       # make targets, all repos
    kartei pick aa: @meth create      # pick, pre-filtered

The repo atom needs whitespace (or end of input) after the colon, so
`Foo::Bar` is never mistaken for one. The `@kind` sigils restrict
the symbol kind (see the TSV contract below for all kinds); several
OR-combine and unique prefixes resolve (`@meth` == `@method`). Terms
are normalized like the index is, hence all-casing comes for free;
each token is also singularized on both sides, so `businessCase`
finds `business_cases`. Space-separated words AND-combine, each
matching the normalized qualified name (scope included) or the
file path the symbol lives in — `lib/remote EntityRef` finds
`EntityReferenceV2` through one path word and one name word.
Matches must start at a token boundary (separators, camelCase
humps, digit transitions): `pain` finds `painless`, `ai` does not.
A word spelled with a slash filters the file path only, so `spec/`
lists the spec tree instead of drowning in symbols merely named
spec; scope-qualified names spell with `::` (`users_api::api_v1
find` finds `UsersApi::ApiV1::Find`). File symbols carry their
whole repo-relative path, so fragments like `spec/support` match
across component boundaries.
Results are ranked by specificity first — symbols matching every
name word with their qualified name beat rows that only matched
through their file path — then whole-token matches (every word a
complete token, `ai` the token `ai` and not the start of `aid`)
beat boundary-prefix ones, then by match quality of the whole term
(exact, prefix, contains), then by kind relevance (class, module,
file, method, func, smethod, const, target, arg, heading, env, key,
image — structural anchors first, data keys and images last; in
pick the first row renders next to the fzf prompt), then by target
length — with a fixed word count a shorter target means a denser
match — then ordered by
(repo, path, line). A file row is dropped once a specific symbol
hit from the same file is listed: that symbol already points at the
file, the row would only echo its path. File rows of other files
stay — they are the only pointer to their file. Path words never
count as specific — they narrow the tree, so searches of path words
alone (eg. `spec/`) keep listing all file rows.

Supported languages: Ruby, Bash/sh, Rust, Erlang, C, Make, YAML,
Markdown, Dockerfile, JavaScript/JSX, TypeScript/TSX, Python — plus
filename and path-token indexing for every tracked file. YAML
anchors, aliases and merge keys are resolved at extraction, so a
key only written under `default: &default` is also indexed as
`production.default...` when merged via `<<: *default` — pointing
at the line where the key is actually written. Performance numbers
live in `docs/BENCHMARKS.md`.

## Configuration

`~/.config/kartei/config.toml` (override with `--config`); the
database lives at `~/.local/share/kartei/index.db` (override with
`--db`).

| Key | Default | Meaning |
|---|---|---|
| `index.paths` | — (required) | start paths searched for git repos |
| `index.nested` | `[]` | directories below which nested git repos (repos within repos, any depth) are indexed as repos of their own; elsewhere the walk stops at the first repo root |
| `index.ignores` | `[]` | files skipped entirely at indexing: path parts (`/fixtures/vcr_cassettes/`), `glob:**/*.snap`, `regex:\.min\.js$` |
| `index.max_file_size` | `2097152` | extraction size cap in bytes |
| `index.resolve_yaml` | `true` | resolve YAML anchors/aliases/merges into inherited key symbols; re-index with `--full` after changing |
| `abbreviations.*` | — | query atom -> repository name |
| `pick.expect` | `btab ctrl-space tab` | keys fzf reports via --expect |
| `pick.landing` | `README.md` | file listed per repo on empty pick input |
| `pick.limit` | `100` | maximum pick candidates per keystroke |
| `pick.highlighter` | `bat --color=always --style=numbers` | preview highlighter; `{file}` placeholder or path appended, one output line per input line; empty = built-in renderer |
| `pick.highlight` | `48;5;238` | ANSI SGR painting the symbol line in the preview; empty disables |
| `pick.match_highlight` | `31` | ANSI SGR painting the matched term parts in the pick list (dark red); empty disables |
| `pick.fzf_args` | `[]` | extra fzf arguments |

## TSV contract (plumbing)

`kartei query` emits one candidate per line, tab-separated, ranked
as described above but printed in reverse — the best hit comes
last, so it ends up next to your shell prompt (`--limit` still
keeps the best N):

    repo  path  line  kind  scope  name  name_norm  qual_norm

* `path` is repo-relative; `line` is 1-based and empty for file rows.
* `scope` is the enclosing context (`Foo::Bar`) or empty.
* `kind` is one of: `class module method smethod const func target
  key heading image arg env file` (`env` covers Dockerfile ENV as
  well as shell/make variable assignments).
* `name_norm` is the normalized leaf name (snake_case folded);
  `qual_norm` the normalized fully-qualified name (scope included) —
  the form name words match against, so `users_api::api_v1 find`
  finds `UsersApi::ApiV1::Find`.

This contract is versioned with the tool — wrappers may rely on it.

## Pick

`kartei pick` runs fzf as a pure display: its own matcher is
disabled and every keystroke re-queries the index with the grammar
above, so the list always shrinks to what matches — with the
pointer snapping back to the best hit (the row next to the prompt)
on every reload. An empty input
shows the landing view — one `pick.landing` file (README.md by
default) per repository, a project browser. Rows render as
`repo@kind  Scope::Name  path:LINE` with a preview pane scrolled
and centered onto the highlighted symbol line; the term
parts a row matched light up (`pick.match_highlight`, dark red by
default) — token-wise, mapped back from the normalized matching, so
`user_policy` highlights `UserPolicy` as a whole.

The selection is printed as `<absolute-path>[:LINE]`. When one of
the configured `pick.expect` keys confirmed the selection, the key
is prepended: `<key>\t<absolute-path>[:LINE]`. Escape clears the
current input back to the landing view; escape on an empty input
aborts. An aborted pick exits with code 130 and no output.

## Example shell wrapper

The successor of a classic `fzf` file-hopping function — dispatching
on the pressed key while kartei owns the candidate stream:

```sh
#!/usr/bin/env bash
# kcode - pick a symbol via kartei and open it.

SELECTION="$(kartei pick "${@}")" || exit 0
KEY="$(printf '%s' "${SELECTION}" | awk -F'\t' 'NF > 1 { print $1 }')"
TARGET="$(printf '%s' "${SELECTION}" | awk -F'\t' '{ print $NF }')"

case "${KEY}" in
  btab)        gvim "${TARGET}" ;;
  ctrl-space)  i3-sensible-terminal-dir "$(dirname "${TARGET%%:*}")" ;;
  tab)         krusader-tab "$(dirname "${TARGET%%:*}")" ;;
  *)           vim "${TARGET}" ;;
esac
```

## Development

    cargo test                        # unit + integration tests
    cargo clippy --all-targets        # must stay warning-free
    cargo fmt --check                 # 80-column house style
    exe/release 0.2.0                 # bump, commit, tag, push

The `examples/` tree in the repository root is the integration
testing ground: a small, fictional multi-repo file tree including a
nested-repository trap (see `examples/README.md` for how the tests
materialize it as git repositories). `docs/ARCHITECTURE.md`
documents the design and the checklist for adding a language.
`exe/release VERSION` bumps the crate version, commits, tags
`vVERSION` and pushes — the tag triggers the GitHub release workflow
that builds and publishes the binaries.

## License

MIT — see `LICENSE`. The vendored Dockerfile grammar
(`vendor/tree-sitter-dockerfile`) keeps its own MIT license file.
