# Smoke check: how to use it

## By hand

```sh
scripts/smoke                 # build, then run every fixture on the VM and natively (~20s)
scripts/smoke --vm-only       # VM only, no LLVM build
scripts/smoke --filter dict   # only fixtures whose path contains "dict"
scripts/smoke --no-build      # reuse the binary already built
scripts/smoke --bless         # record the VM's output as each fixture's golden
```

- Fixture list: `tests/smoke/fixtures.txt`, one `<mode> <path>` per line.
  - `run`: the VM exits 0 and matches its golden (`tests/smoke/expected/`),
    and the native output matches the VM's.
  - `vm`: the same, VM only.
  - `reject`: `mojito check` fails with the fixture's `# expect:` text.
- Exit status: 0 all passed, 1 a fixture failed, 2 the build failed.
- Report: printed, and kept in `target/smoke/last.log`.
- To add a fixture: append a line to `fixtures.txt`, then
  `scripts/smoke --bless --filter <name>`.
- No known failure at the start: the native `collection_display_default`
  failure (R1091) is fixed, so a run from a clean tree reports 0 failed.

## In claude_loop.py (on by default)

Every run that executes tasks runs the smoke check with no extra flag:

1. A baseline run before the first task records what already fails.
2. After each executed task, the loop runs it again. A task is blamed only
   for failures it added; the result then becomes the next task's baseline.
3. A task that added a failure gets its own session resumed with the report
   to fix it, committed separately. If it still fails, the task counts as
   having left work behind. Reports: `target/claude-loop/<time>-<ID>-smoke.log`.

Flags:

```sh
--no-smoke          # turn the check off
--smoke-vm-only     # skip the native backend
--smoke-fix N       # repair attempts per task (default 1; 0 only reports)
```

It does not run under `--dry-run` or `--plan`.

## Commit first

`scripts/smoke` and `tests/smoke/` are new, untracked files. The loop only
auto-commits tracked changes, so commit them before the next loop run.

## Next run

R1037 through R1081 have all landed, so
`scripts/claude_loop.py --start R1037 --until R1081 -n 0` stops at once with
"no entry R1037 on the roadmap". The next steps, which do run the smoke check:

```sh
scripts/claude_loop.py --only R1094 -n 0
scripts/claude_loop.py --only R973 --only R974 --only R1039 --only R975 \
    --only R1032 --only R1033 --only R1034 -n 0
```
