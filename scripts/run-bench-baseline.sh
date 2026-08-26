#!/usr/bin/env bash
# Capture the pinned-runner benchmark baseline (procedure:
# benchmarks/native/noise-policy.md). Run on a QUIET machine with the CPU
# governor pinned; restore the governor afterwards. Expect ~10-20 minutes
# (release builds of the compiler and runtime, then the full corpus twice).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

RUNNER_ID="${RUNNER_ID:-$(hostname)}"
BASELINE_DIR="benchmarks/native/baseline/${RUNNER_ID}"
mkdir -p "${BASELINE_DIR}"

echo "==> pinning CPU governor (sudo; restore with: sudo cpupower frequency-set -g schedutil)"
sudo cpupower frequency-set -g performance

echo "==> run 1 (agreement check)"
scripts/bench-pliron --summary "${BASELINE_DIR}/summary.run1.tsv" \
    --raw "${BASELINE_DIR}/raw.run1.jsonl"

echo "==> run 2 (the baseline, if it agrees with run 1)"
scripts/bench-pliron --summary "${BASELINE_DIR}/summary.tsv" \
    --raw "${BASELINE_DIR}/raw.jsonl" \
    --check "${BASELINE_DIR}/summary.run1.tsv"

echo "==> runs agree within the noise policy; baseline at ${BASELINE_DIR}/summary.tsv"
echo "    Commit summary.tsv + raw.jsonl; delete the run1 files."
echo "==> restoring governor"
sudo cpupower frequency-set -g schedutil
