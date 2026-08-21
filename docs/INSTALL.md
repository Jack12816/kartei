# Installation

## Build

A stable Rust toolchain and a C compiler are required (the tree-sitter
grammars and bundled SQLite compile from source); fzf is needed at
runtime for `kartei pick` only.

    cargo install --path .

This drops the binary at `~/.cargo/bin/kartei`. Alternatively build
with `cargo build --release` and copy `target/release/kartei` to
`~/.local/bin` (the path the shipped systemd unit expects).

## Configuration

    kartei init
    $EDITOR ~/.config/kartei/config.toml

Set `index.paths` to the directories holding your git repositories
and add your `[abbreviations]`. See the README for the full config
reference.

## First run

    kartei index

The first pass builds the whole index (about 2 s per ~9k files, see
`docs/BENCHMARKS.md`); every later pass is incremental and costs
milliseconds when nothing changed. The database lives at
`~/.local/share/kartei/index.db`.

## systemd user timer

Copy the shipped units and enable the timer:

    mkdir -p ~/.config/systemd/user
    cp systemd/kartei-index.{service,timer} ~/.config/systemd/user/
    systemctl --user daemon-reload
    systemctl --user enable --now kartei-index.timer

The timer runs 2 minutes after login and then every 3 minutes after
the previous run started — an interval backed by the benchmarks: an
idle pass costs well under a second even at a couple hundred
repositories. `Persistent=true` catches up after suspend; systemd
never overlaps runs, and the indexer additionally holds a flock, so
manual `kartei index` runs are always safe.

Inspect runs with:

    systemctl --user list-timers kartei-index.timer
    journalctl --user -u kartei-index.service -n 20

If the binary lives elsewhere than `~/.local/bin`, adjust `ExecStart`
in the service unit.

## cron fallback

On hosts without systemd, a crontab entry with a non-blocking flock
gives the same behavior (the extra flock guards against overlap when
a run ever outlasts the interval):

    */3 * * * * flock -n ~/.local/share/kartei/index.cronlock kartei index
