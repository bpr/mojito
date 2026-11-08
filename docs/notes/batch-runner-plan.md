# Batch runner: generalize `scripts/pmir-batches` and add a compile step

Status: planned, not started. Do it after the current `scripts/pmir-batches`
run finishes, since that run reads its own script and commits as it goes.

## Why

The work strategy is a tiered check around batches of roadmap tasks:

| tier | time | runs | catches |
|---|---|---|---|
| `scripts/smoke` | ~30–40 s | `claude_loop.py`, after every task | gross breakage, both backends |
| `scripts/focused` | ~4 min | after every batch | the fragile-area fixtures, every corpus group and natively |
| instantiation ratchet | ~1–2 min | after every batch | a fixture that stops elaborating; growth over 1% |
| overnight gate | ~9 h | nightly | all suites, the corpus binary, the pliron manifests |

`scripts/pmir-batches` drives that for the `pmir` track only, and
the fast tiers miss a test target that stops compiling:
`tests/pliron_backend_test.rs` did not compile for weeks
(`TyKind::Error` was removed), which hid three real bugs, found
2026-11-08.

## 1. Generalize the batch script

Today `scripts/pmir-batches` reads its batches from the `claude_loop.py`
commands in `docs/notes/representation-consolidation.md`. Make one script
(say `scripts/batches`) that takes its batches from either source:

- `--note PATH`: the `claude_loop.py` commands in a note, one batch each,
  as now.
- `--track T [--size N]`: the track's open entries, ordered by the
  importance metric below, cut into batches of `N`.

Keep everything else `pmir-batches` does: each batch's loop, focused, and
ratchet; the stop on a lost fixture or growth over `--max-growth`; the
baseline refresh commit; `--start N` and `--dry-run`; the one-line in-place
progress display with full output only in the per-step logs.

Leave `scripts/pmir-batches` as a thin wrapper (`--note
docs/notes/representation-consolidation.md`), or delete it once nothing
names it.

## 2. Add a compile-only step to each batch

After focused and before the ratchet, run

```sh
cargo build --tests --features backend-pliron
```

(the native lane, through `scripts/llvm-prefix`), so every test target,
the `--features backend-pliron` ones included, has to compile. It takes
seconds once the build is warm. A failure stops the run like a focused
failure, so a broken test file is found at the batch that broke it.

## 3. Importance metric for ordering a track

The `pmir` batches came from asking for the track sorted by an importance
metric: the entries likely to fix others, with a cascading effect. That is
hard to judge in general; a quick approximation is the number of open
entries that depend on an entry, directly or transitively, through their
`Depends on` bullets.

- Add `scripts/roadmap.py rank [--track T]`: open entries by transitive
  dependent count, ties kept in work order (`roadmap.py list`).
- The `--track` mode of the batch script orders by this, while still
  putting each entry's open prerequisites ahead of it, as `list` does.
- Judgment can refine it (an entry whose probes name other entries'
  symptoms also cascades), but the count is the default.
