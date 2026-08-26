# Native benchmark noise policy

The authority for how benchmark numbers may be interpreted and when a
comparison counts as a regression. The `--check` thresholds in
`tools/bench/src/main.rs` (`CHECK_THRESHOLDS`) mirror this table; change
both together, as a reviewed policy change.

## Pinned acceptance runner

Budget enforcement (`scripts/bench-pliron --check`) is only meaningful on
the pinned runner recorded in the baseline's leading JSONL metadata record
(CPU model, governor, toolchain versions, git revision). Developer runs on
other machines are comparison-only; they must not update the baseline or
be quoted as acceptance results.

Baseline capture procedure:

1. Quiet machine (no builds, browsers, or VMs competing).
2. CPU frequency pinned: `sudo cpupower frequency-set -g performance`
   (restore `powersave`/`schedutil` afterwards); the runner metadata
   records the governor so an unpinned baseline is visible.
3. `scripts/bench-pliron --summary benchmarks/native/baseline/<runner-id>/summary.tsv --raw .../raw.jsonl`
   twice; the two summaries must agree within the conclusiveness bound
   below before the second is committed as the baseline.

`<runner-id>` is a short stable name for the machine (e.g. hostname).

## Sampling

- Compile lane: 2 warmups, ≥5 recorded samples per fixture/profile.
- Execution and VM lanes: 2 warmups, ≥10 recorded samples.
- Aggregation is median and MAD (median absolute deviation); means are
  never used — one descheduled sample must not move the result.

## Conclusiveness

A measured comparison is *conclusive* only when, for both sides,
`MAD / median ≤ 0.05` for wall-time metrics and `≤ 0.02` for memory and
size metrics. Inconclusive comparisons are reported as such and must be
re-run on a quieter machine rather than being cited either way.

## Regression thresholds (`--check`)

A metric regresses only when **all three** hold: the median exceeds the
baseline median by more than the relative allowance; the delta exceeds the
absolute noise floor; and the delta exceeds `3 × (MAD_base + MAD_new)`.

| metric | relative allowance | absolute floor |
|---|---|---|
| `exec_wall_us` | 10% | 1 ms |
| `compile_wall_us` | 15% | 10 ms |
| `exec_maxrss_kb` | 10% | 1 MiB |
| `compile_maxrss_kb` | 10% | 10 MiB |
| `exe_bytes` | 5% | 4 KiB |

A fixture missing from the current run always fails the check (coverage
loss is a failure, not noise).

These are *detection* thresholds for the scheduled lane; the Stage 6
acceptance budgets (geomean speedups, compile-time and size ratios) live
in `docs/notes/pliron-stage6-plan.md` and are evaluated over the summary
files, not per-sample.
