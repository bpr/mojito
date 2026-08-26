# Pliron Stage 6 — Optimization and Distributable Artifacts (record)

Implementation record for Stage 6 (plan: `pliron-stage6-plan.md`). The
sections below are the binding policy decisions as landed; changing any of
them is a reviewed policy change. Status: implementation complete; the
acceptance-evidence record is at the end, with only the upstream-blocked
dependency-upgrade rehearsal still open.

## Optimization profiles and pipelines

`src/backend/pliron/pipeline.rs` is the single owner of optimization
policy. Public profiles: `O0` (default) and `release` (`--native-opt
release`, with `1` a permanent alias). Both share the pliron cleanup stage
— per-function `mem2reg` then `dce`, run between whole-module
verifications; profiles diverge only at the LLVM stage, where `release`
runs `opt -passes=default<O1>` over emitted bitcode out of process
(pliron-llvm 0.17 keeps its `LLVMModuleRef` private). `Pipeline::describe`
is pinned by expect-tests, so any pass-list or pipeline-string change is a
deliberate diff. `default<O2>` was not evaluated; adopting it requires the
benchmark gate below. `MOJITO_PLIRON_VERIFY_EACH_PASS` verifies around
every individual pass (the instrumented lane); production verifies before
and after the whole pipeline.

Analysis invalidation is tested against pliron 0.17's real
`AnalysisManager` (`src/backend/pliron/pipeline.rs` tests): a mutating
pass that does not preserve evicts, preserved-or-unchanged retains, a
wrongly-preserving pass demonstrably serves stale data, and a change
inside the production `NestedOpsPass` boundary evicts module-level
analyses (eviction is by analysis type across all op keys). pliron 0.17
has no analysis-dependency tracking and no per-scope analysis-manager
hierarchy; the tests are scoped to what exists.

## Toolchain and tool pins

`src/backend/pliron/toolchain.rs` resolves `clang`/`opt` to absolute PATH
entries once, requires LLVM major 22 (the `llvm-sys = "221"` pin), and
validates the runtime archive — provenance, sha256, and the
`mjrt_abi_version` value read mechanically from the archive — before any
frontend work (`check_toolchain`). `--print-toolchain` reports it all as
stable `key\tvalue` lines. Tool invocations pin `LC_ALL=C`, pass
`--no-default-config` and an explicit `--target`, and executable links add
`-Wl,--build-id=none`; the clang argument order is snapshot-pinned.

Runtime discovery order: `--runtime-lib` → `MOJITO_RUNTIME_LIB` →
installation bundle (`<exe>/../lib/`) → development target tree. Explicit
steps naming a missing file are hard errors, never fallthroughs.

## Debug information policy

`--native-debug lines` (the default; `none` opts out) attaches DWARF 5
subprograms and call-granular line locations at emission time:
pliron-llvm 0.17 drops locations at conversion, so
`src/backend/pliron/debug.rs` reparses the stamped IR text into a private
`llvm-sys` context and uses the C DIBuilder — no fork. Correlation rests
on `CallOp`/`CallIntrinsicOp` being the only call-instruction producers;
a per-function count-and-same-file assertion degrades to subprogram-only
(never a wrong line), and `pliron_debug_test` pins zero degradations
corpus-wide. `DIFile` names are the compilation's source labels —
relative labels exactly as given, an absolute CLI path degraded to its
file name — with an empty compile-unit directory, so no absolute or
temporary path is ever embedded (test-pinned). Textual
IR and the JIT never carry debug info. Backtraces resolve exact
file:line at `O0` and retain call-site lines at `release` (inlining may
merge frames). Policy: debug info stays in the emitted image at both
profiles; stripping (and separate debug files) is the deployer's choice —
no strip emission mode exists.

## Artifact contract

Every binary artifact is written failure-atomically (temp + rename in the
destination directory; failures preserve prior outputs). `--emit obj`
objects contain the synthesized C `main` wrapper and ship a sidecar
`<obj>.link.tsv` (schema, target, ABI version, object and runtime sha256,
ordered libs, clang major); `mojito link OBJ -o EXE` validates every field
against the resolved toolchain and issues the same deterministic link
line. Executables are self-contained PIEs: x86-64, non-executable stack,
dynamic dependencies limited to libc/libm/libgcc_s/ld-linux, the
`mjrt_abi_version` anchor exported (all test-pinned via
`src/backend/pliron/inspect.rs`, the pure-Rust `object`-crate reader).

## Reproducibility

Two fully independent clean builds are byte-identical for bitcode,
objects, link manifests, and executables at both profiles, debug info
included (`pliron_repro_test`, with per-section sha256 diffs on
mismatch). Known residual nondeterminism: the *textual* `--emit ll`
output at `release` embeds the PID-unique scratch bitcode path in its
`; ModuleID =` comment line (text-only; no binary artifact is affected).
Development-built artifacts legitimately embed cargo paths in the runtime
archive's DWARF; bundle builds remap them (`--remap-path-prefix`) and the
packaged compiler and runtime carry no build-tree paths.

## Distribution

`scripts/package-pliron` produces `dist/mojito-<version>-<triple>/`
(`bin/mojito`, `lib/libmojito_runtime.a` + `lib/runtime-link.tsv`,
`share/mojito/stdlib` — the compiler's bundled-support fallback resolves
`<exe>/../share/mojito` when no development tree exists —
`share/mojito/smoke`, `share/doc`, `manifest.tsv`, `checksums.sha256`)
plus a deterministic tar (`--sort=name --mtime=@0 --owner=0 --group=0`,
`gzip -n`), and smoke-compiles and runs a program from the assembled
bundle in a clean environment. `pliron_dist_test` proves relocated-bundle
discovery (paths with spaces/non-ASCII), bundle-stdlib service with the
development tree hidden behind a mount namespace, `--runtime-lib`
precedence, empty-environment execution, and actionable rejection of
missing, corrupt, and ABI-mismatched runtimes.

## Benchmarks

`benchmarks/native/` holds the nine-category corpus (differentially
validated at both profiles), `manifest.tsv`, and `noise-policy.md` (the
authority for sampling, MAD conclusiveness, and regression thresholds;
`tools/bench` mirrors its `--check` thresholds). `tools/bench`
(`mojito-bench`, outside the workspace) measures compile wall/phase
times (`--timings`), peak RSS via `wait4`, artifact sizes, executable
runtime, and the VM baseline; raw JSONL with a runner-metadata record,
median/MAD TSV summaries, a seeded-slowdown self-test, and the
`check-pliron` smoke subset. `scripts/bench-pliron` is the release-build
performance lane; `run-bench-baseline.sh` scripts the pinned-runner
capture (two agreeing runs, governor pinned).

## Dependency-upgrade rehearsal

Not executable at close: `pliron`/`pliron-llvm` 0.17.0 and `llvm-sys`
221.0.1 are the latest published releases as of 2026-08-24. The rehearsal
procedure is: branch, bump the pins, run `scripts/check-pliron`, the full
parity + sanitizer lanes, `pliron_repro_test` (reproducibility across the
upgrade), and `scripts/bench-pliron --check` against the committed
baseline; record required patches, effort, and output changes; discard
the branch. It must run — and pass — on the first upstream release
before that release is adopted, and its record feeds the promotion
decision.

## Acceptance evidence

Recorded 2026-08-26 on the pinned runner (`pop-os`); the
dependency-upgrade rehearsal is the only item still open:

1. Pinned-runner baseline committed under
   `benchmarks/native/baseline/pop-os/` (two agreeing governor-pinned
   runs per the noise policy). Budget evaluation
   (`scripts/stage6-budget-report` over `summary.tsv`): release 1.40x
   over `O0` by exec geomean (budget 1.15x) and far past the VM budget;
   no fixture regresses >10% at release; compile median release 1.04x
   worst-case (budget 2.0x); peak RSS 1.02x worst-case (budget 1.5x);
   exe-size geomean 1.00x (budget 1.10x, unstripped upper bound);
   startup within allowance. Waivers: the "O0 within 1.15x of the
   pre-Stage-6 compile baseline" line is N/A — no pre-Stage-6 baseline
   was ever captured, so this baseline is the first reference point; and
   a handful of micro-metrics (sub-ms startup walls, few-KB RSS medians)
   jitter above the MAD conclusiveness bound, but every affected verdict
   passes with margin several times the jitter.
2. Full gates: `scripts/check` (3322/3322 workspace tests, fmt, clippy)
   and `scripts/check-pliron` (344/344 pliron-lane tests, spike gate
   11/11, bench smoke) — the complete parity + sanitizer lanes at both
   profiles run inside the pliron gate's manifest test.
3. Container-based compiler-free execution: the packaged bundle compiled
   the smoke fixture and the executable ran in a bare `ubuntu:24.04`
   container printing `mojito bundle ok` (the automated approximation —
   `env -i` + DT_NEEDED inspection — remains in `pliron_dist_test`).
4. The dependency-upgrade rehearsal, blocked on an upstream release
   (procedure above).
