# Benchmarks

Measured 2026-08-01 with hyperfine on a real-world working tree
of 31 git repositories — 8,748 tracked files, 99,818 symbol rows,
16 MB database — release build, 24-core container. Methodology: 5-20
runs per case; the single-change case appends a line to a Ruby model
(making the repo dirty, as a real edit would) and restores it
afterwards.

## Indexing

| Case | Wall time (mean ± σ) | CPU |
|---|---|---|
| Initial full index (cold DB) | 1.770 s ± 0.059 s | 13.6 s user |
| Incremental, nothing changed | 89.5 ms ± 3.1 ms | 0.03 s user |
| Incremental, one file changed | 141.6 ms ± 5.2 ms | 0.07 s user |

Notes:

* The no-op run cost is dominated by the per-repo git subprocesses
  (HEAD + dirty check), ~3 ms per clean repository. Extrapolated to
  ~200 repositories that is roughly 0.6 s per idle pass.
* A content edit re-extracts exactly the changed file: the repo's
  `ls-files` + stat sweep plus one parse and one transaction.
* Mtime-only changes (eg. `touch`) leave the worktree git-clean, so
  the repository is skipped entirely — correct, since the content is
  unchanged.
* Caching compiled tree-sitter queries per process (instead of
  compiling per file) brought the initial index from 13.4 s down to
  1.8 s — the single biggest win; parsing itself is not the
  bottleneck.

## Querying

| Case | Wall time (mean ± σ) |
|---|---|
| `query 'acme-api: user policy'` (repo + term filter) | 4.7 ms ± 0.5 ms |
| `query user` (term over all repos) | 17.6 ms ± 1.6 ms |
| `query` (full 100k-row dump, the pick stream) | 213.3 ms ± 11.0 ms |

Interactive feel: `pick` shows the fzf UI in well under a quarter
second even when streaming every symbol; repo-filtered picks are
effectively instant.

## Timer interval conclusion

An idle pass costs ~90 ms here and an extrapolated ~0.6 s at 200
repositories — a 3-minute timer (`OnUnitActiveSec=3min`, the shipped
default) produces a duty cycle far below one percent, and even a
1-minute interval would be entirely harmless. Intervals below 1
minute buy nothing: freshness is bounded by editing pace, not by
indexing cost.
