# Committed benchmark baselines

One directory per pinned runner (`<runner-id>/summary.tsv` plus its
`raw.jsonl`), captured with the procedure in `../noise-policy.md`. The
leading JSONL record documents the machine, governor, toolchain, and git
revision the baseline was measured at. `scripts/bench-pliron --check
benchmarks/native/baseline/<runner-id>/summary.tsv` enforces regressions
against it on that runner only.
