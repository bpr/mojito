#!/usr/bin/env bash
# Capture the pinned-runner benchmark baseline (procedure:
# benchmarks/native/noise-policy.md). Run on a QUIET machine with the CPU
# governor pinned; the prior governor is restored on exit, success or not.
# Expect ~10-20 minutes (release builds of the compiler and runtime, then the
# full corpus twice).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

RUNNER_ID="${RUNNER_ID:-$(hostname)}"
BASELINE_DIR="benchmarks/native/baseline/${RUNNER_ID}"
mkdir -p "${BASELINE_DIR}"

# Pin the governor through sysfs: cpupower is not installed everywhere, and
# the governor set depends on the driver (intel_pstate offers only
# performance/powersave, so a hardcoded restore target cannot work). Already
# pinned means no sudo is needed at all.
GOVERNOR_FILE=/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor
PREVIOUS_GOVERNOR=$(cat "${GOVERNOR_FILE}")
restore_governor() {
    echo "==> restoring governor ${PREVIOUS_GOVERNOR}"
    echo "${PREVIOUS_GOVERNOR}" \
        | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor >/dev/null
}
if [[ "${PREVIOUS_GOVERNOR}" != performance ]]; then
    echo "==> pinning CPU governor to performance (sudo; was ${PREVIOUS_GOVERNOR})"
    echo performance \
        | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor >/dev/null
    trap restore_governor EXIT
fi

echo "==> run 1 (agreement check)"
scripts/bench-pliron --summary "${BASELINE_DIR}/summary.run1.tsv" \
    --raw "${BASELINE_DIR}/raw.run1.jsonl"

echo "==> run 2 (the baseline, if it agrees with run 1)"
scripts/bench-pliron --summary "${BASELINE_DIR}/summary.tsv" \
    --raw "${BASELINE_DIR}/raw.jsonl" \
    --check "${BASELINE_DIR}/summary.run1.tsv"

echo "==> runs agree within the noise policy; baseline at ${BASELINE_DIR}/summary.tsv"
echo "    Commit summary.tsv + raw.jsonl; delete the run1 files."
