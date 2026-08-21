# Architecture

This document distills the design decisions behind kartei so future
maintenance does not need to re-derive them. The full research trail
(candidate comparisons, rejected options, citations) lives in the
repository root's RESEARCH.md; the implementation plan in PLAN.md.

## The shape of the tool

kartei is two programs sharing one database:

1. **The indexer** (`kartei index`) — a timer-driven batch job that
   discovers git repositories, extracts symbols and keeps the
   database current.
2. **The query side** (`kartei query` / `kartei pick`) — an
   interactive, millisecond-startup reader that never blocks on the
   indexer.

The split follows git's plumbing/porcelain model: `query` emits a
stable TSV contract for any wrapper; `pick` is a thin porcelain over
the same code path that owns the fzf wiring.

## One SQLite file, no search engine

All state — repositories, per-file mtimes and the symbol rows — lives
in a single SQLite database in WAL mode. This was a deliberate
decision against a dedicated search engine (tantivy, meilisearch):

* A per-file re-index is one transaction (delete rows, insert rows,
  update mtime) — the bookkeeping can never disagree with the index,
  which a two-store split cannot guarantee across crashes.
* WAL gives the exact concurrency topology needed: one batch writer,
  any number of interactive readers, readers never blocked.
* At the expected scale (a few hundred repos, well under ~2M symbol
  rows) indexed lookups are single-digit milliseconds; nothing a
  search engine offers pays for its operational surface here.

The interactive UI is delegated to fzf, but as a pure display: its
own matcher is disabled and every keystroke reloads the candidate
list from the database (`kartei candidates`), so one grammar
(`[repo:] [@kind ...] [term words ...]`) drives plumbing and
porcelain identically and the database owns matching and ranking
(exact, prefix, contains). Match highlighting therefore also moves
into the feed: the span-aware tokenizer behind the normalizer maps
every normalized word occurrence back onto the raw display text
(whole tokens — singularization forbids an exact character mapping)
and paints those spans with a configurable SGR; fzf just renders
them via --ansi and strips the codes from placeholders and the
selection output.

## Normalization: all-casing at index time

Every symbol name is stored twice: verbatim (`name`) and normalized
(`name_norm`). The normalizer splits on separators (`_ - . / :` and
whitespace), camelCase humps, acronym boundaries and letter-digit
transitions, lowercases and singularizes each token and joins with
underscores — so `FooBar`, `foo_bar`, `foo-bar` and `Foo::Bar` share
one key, and `UserPolicies` matches `user_policy`. Query terms get
the identical treatment, which makes case- and plural-variant
matching a non-feature at query time instead of a query-side
expansion problem.

The singularizer is deliberately naive: because both sides run the
same rules, the canonical form never has to be correct English —
but since matching is substring-based, the rules may only ever keep
too many characters (`buses` -> `buse` still contains `bus`), never
strip too many (`cases` -> `cas` would lose `case`). Hence exactly
one trailing `s` is stripped, `ss` endings are kept, and `ie`/`ies`
endings unify to `y` (`cookie` == `cookies`).

Matching anchors at token boundaries: an occurrence must start at
the string start or right after a separator-collapsed underscore,
while its tail may end mid-token — `pain` finds `painless`, `ai`
does not, and `entity_ref` still spans from the `entity` boundary
into the `reference` token. Every token start (separators,
camelCase humps, acronym and digit transitions) is an entry point;
mid-token fragments are noise, not matches. The ranking prefers
whole-token occurrences (both ends on boundaries) over
boundary-prefix ones and breaks remaining ties by target length,
since with a fixed word count a shorter target carries the denser
match.

File paths get the same treatment: each file contributes one
`file`-kind symbol row whose name is the whole repo-relative path
and whose `name_norm` is that path normalized into one contiguous
form. Path fragments therefore match across component boundaries —
`spec/support` finds `spec/support/vcr.rb` like any symbol. On the
query side the slash doubles as a scoping marker: a term word
spelled with one filters the file path only, while the `::`
spelling — which normalizes to the same key — keeps matching the
qualified symbol name. `/` means path, `::` means scope, one
canonical form underneath.

## Incremental indexing

The pipeline per run:

1. Take a non-blocking flock beside the database; a second indexer
   exits silently (the running one wins).
2. Discover repositories: walk the start paths, prune at every
   directory containing `.git`. Pruning implements the nested-repo
   rule — throwaway clones inside a repository's tree never become
   index candidates. Directories listed in `index.nested` are the
   exception: below them the walk continues through every repository
   root, so repositories within repositories (at any depth) are
   indexed as repositories of their own, named after the enclosing
   repository plus their path within it (`kartei/kartei`) — the
   basename alone would collide with the parent in the classic
   checkout-in-checkout layout and resolve targets against the wrong
   root. Their files are not tracked by the enclosing repository, so
   nothing is indexed twice.
3. Per repository, cheapest check first:
   * HEAD unchanged and worktree clean on both sides -> skip without
     touching a single file.
   * Otherwise diff `git ls-files -z --format='%(objectmode) %(path)'`
     (gitlinks and symlinks dropped) against the stored per-file
     mtime/size state.
4. Extract changed files in parallel (rayon), one pure function per
   file returning row batches; a single writer commits one
   transaction per file.
5. Deleted files and vanished repositories cascade away via foreign
   keys.

Extraction failures degrade per file (stderr note, bare file row) —
one broken file never aborts a run.

The database is a disposable cache: a schema-version bump drops and
recreates everything instead of migrating, because the repositories
are the source of truth and a full rebuild is cheap.

## Symbol extraction

Extraction is tags-level — names, kinds, lines, a lexical scope
string — not semantic resolution. That is the proven sweet spot for
code navigation (GitHub's search-based code nav works this way);
cross-file reference resolution is explicitly out of scope.

Grammars are tree-sitter, compiled into the binary; there is no
runtime parser dependency. Each language module owns its extraction
end to end: usually an embedded `.scm` definition query (vendored
from the grammar's official tags.scm where one exists, extended where
lacking) plus a small post-processing step for scope chains or path
building. The Dockerfile grammar is vendored as C sources under
`vendor/` because its crate release pins an incompatible tree-sitter
core; `build.rs` compiles it like any other translation unit.

Languages: Ruby, Bash, Rust, Erlang, C, Make, YAML, Markdown,
Dockerfile, JavaScript (incl. JSX), TypeScript (incl. TSX), Python.
Extensionless files are detected via their shebang line; binaries are
skipped via a NUL sniff in the first 4KiB; oversized files keep their
file row but skip extraction.

## Query language

A query is `[repo-or-abbrev:] [term]`. The atom resolves through the
configured abbreviation map, then exact repository names, then unique
name prefixes — and requires whitespace (or end of input) after the
colon so `Foo::Bar` stays a plain term. Filters stay `key:value`
shaped so future atoms (`kind:`, `lang:`) fit without a grammar
rewrite.

## Adding a language — checklist

1. Add the grammar crate to Cargo.toml (prefer crates depending on
   `tree-sitter-language`; vendor C sources like Dockerfile when the
   crate pins an old core).
2. Add a variant to `Lang` in `src/lang/mod.rs`: `as_str`, the
   `detect` rule (filename/extension/shebang) and the `extract`
   dispatch arm.
3. Write `src/lang/<lang>.rs` with `pub fn extract(source: &[u8])`,
   usually via `run_query` plus an embedded
   `src/lang/queries/<lang>.scm`.
4. Add a fixture under `tests/fixtures/<lang>/` and in-module tests
   asserting exact `(line, kind, name, scope)` tuples — these double
   as the canary against node-name drift when grammar crates get
   bumped.
5. Extend the kind vocabulary only when nothing existing fits; kinds
   are part of the TSV contract.
6. Document the language in the README table and here.
