# Pliron Stage 6: Optimization and Distributable Artifacts — Implementation Plan

## Objective

Turn the feature-gated Pliron backend from a semantics-complete experimental
backend into a measured, reproducible native toolchain candidate. Stage 6 owns
optimization policy, pass correctness, source-level debug locations, object and
executable linking, runtime packaging, reproducibility, and benchmark evidence.
It does not decide Pliron promotion by itself; it produces the evidence consumed
by the separate promotion decision in [`docs/roadmap.md`](../roadmap.md).

The VM remains the executable semantic oracle, verified post-drop MIR remains
the backend-independent waist, and the default build remains free of Pliron and
LLVM dependencies.

## Current Baseline

- `native::target::OptLevel` exposes `O0` and `O1`.
- Every compilation already runs Pliron mem2reg and DCE, with verification
  before and after the pass pipeline.
- `O1` shells out to LLVM 22 `opt -passes=default<O1>` after LLVM conversion.
- LLVM IR, bitcode, object, and executable emission exist. Object and executable
  emission currently shell out to Clang 22; executables statically link the
  separately built `libmojito_runtime.a` found through `MOJITO_RUNTIME_LIB` or
  development-tree discovery.
- Pliron locations preserve source name and line/column in textual Pliron IR,
  but source locations are not yet translated into LLVM debug metadata.
- `conformance/pliron-parity.tsv` has zero supported-language exclusions and
  tests VM/native output, error, and lifecycle parity at both optimization
  levels. Sanitizer and native ABI symbol checks already exist.
- There is no checked-in benchmark corpus, benchmark runner, distribution
  layout, installed-runtime lookup contract, or artifact reproducibility gate.

## Decisions to Freeze Before Implementation

Land the Stage 6 design note before changing optimization or emission. Record
these decisions in it and treat later changes as reviewed policy changes:

1. **Optimization profiles.** Keep `O0` as the debuggable baseline and replace
   the public `O1` spelling with a named `release` profile only if CLI
   compatibility is preserved. Internally, use a profile-to-pipeline table;
   do not scatter `OptLevel` conditionals through lowering and emission.
2. **Pipeline ownership.** Pliron owns target-independent cleanup and any
   Mojito-specific rewrites. LLVM owns target-specific release optimization.
   The selected LLVM pipeline, target flags, and tool versions are explicit
   build inputs.
3. **Artifact contract.** A distributable executable is a normal host ELF that
   contains the Mojito runtime and needs neither `mojito`, LLVM, Pliron, Clang,
   nor a separate runtime library at execution time. A relocatable object is a
   developer artifact and ships with an explicit runtime-link manifest and the
   matching runtime archive.
4. **Compatibility.** Keep the existing link-time reference to the versioned
   runtime ABI symbol. Add an executable startup check only if static-link
   version pinning cannot produce a clear diagnostic for every supported
   mismatch scenario; do not create a second ABI version authority.
5. **Reproducibility.** Release artifacts are byte-identical for two clean
   builds from the same source tree, toolchain, target, profile, and declared
   environment. If an upstream ELF section cannot be stabilized, identify it
   mechanically, document why, compare normalized artifacts, and keep all
   loadable/code/data sections byte-identical.
6. **Debug scope.** Stage 6 requires source file, line, and column on emitted
   functions and executable instructions sufficient for debugger backtraces.
   Full variable inspection and optimized stepping are follow-up work unless
   they fall out naturally from LLVM metadata support.

## Benchmark Contract and Pre-declared Budgets

Create the benchmark corpus and commit an `O0`, release, and VM baseline before
landing new optimization passes. Run benchmarks on a documented pinned Linux
x86-64 runner with CPU frequency controls, warmups, repeated samples, median
and median absolute deviation, and recorded compiler/LLVM/Clang/Pliron versions.
Raw samples and the summarized JSON/TSV result must be retained as CI artifacts.

The corpus must contain at least:

- scalar arithmetic and branch-heavy loops;
- direct calls and recursion;
- strings and formatting;
- struct, tuple, Variant, and collection-heavy programs;
- allocation and pointer traffic;
- iterator and SIMD workloads;
- exceptions and destructor-heavy control flow;
- one small startup-dominated executable and one larger whole-program fixture.

Freeze the following initial acceptance budgets before optimization changes.
Recalibration requires an explicit roadmap/design-note change, never a silent
test update.

| Metric | Stage 6 budget |
|---|---|
| Semantic correctness | Complete parity suite passes at `O0` and release; sanitizer and lifecycle lanes remain clean. |
| Runtime benefit | Release is at least 15% faster than `O0` by geometric mean and at least 25% faster than the VM by geometric mean on the runtime corpus. |
| Runtime regressions | No benchmark is more than 10% slower than `O0` unless the absolute change is below the runner's pre-recorded noise floor and is waived in the Stage 6 note. |
| Native compile time | Release median is no more than 2.0x `O0`; `O0` is no more than 15% slower than the checked-in pre-Stage-6 baseline. Measure frontend, MIR, Pliron lowering/passes, LLVM conversion/optimization, object emission, and link separately. |
| Peak memory | Release peak RSS is no more than 1.5x `O0`, and no corpus case exceeds a separately declared runner ceiling. |
| Code size | Release geometric-mean stripped executable size is no more than 1.10x `O0`; no individual artifact is more than 1.25x without a documented waiver. Record Pliron text, LLVM IR/bitcode, object text/data, runtime archive contribution, and final stripped/unstripped ELF sizes. |
| Startup | Release process startup is no more than 5% or 1 ms slower than `O0`, whichever allowance is larger. |
| Reproducibility | Two clean same-input release builds match byte-for-byte, or only the pre-declared, mechanically reported nondeterministic sections differ. |

Use statistical confidence/noise checks to reject inconclusive performance
claims. A failed performance budget is a Stage 6 failure even when correctness
passes; retain `O0` or disable the offending release pass instead of weakening
semantic or MIR contracts.

## Ordered Implementation

### S6.1 — Measurement foundation

1. Add a small standalone benchmark driver under `tools/` or `scripts/` so it
   does not affect the default library dependency graph. It must build each
   fixture once per profile, separate compile/link timing from execution, run
   binaries outside Cargo, capture peak RSS and artifact sizes, and emit a
   stable machine-readable schema.
2. Add benchmark sources under a dedicated `benchmarks/native/` tree. Keep them
   valid Mojo programs and run them through the same `Compiler` and cached
   elaborated MIR path as normal compilation.
3. Check in the benchmark definition, runner metadata schema, noise policy, and
   pre-optimization baseline. Do not check in machine-specific claims as
   universal results; distinguish the pinned acceptance runner from developer
   comparison runs.
4. Add a quick smoke subset to `scripts/check-pliron`; put stable performance
   enforcement in a dedicated scheduled/manual `scripts/bench-pliron` lane so
   noisy PR runners do not produce false failures.

Exit: repeated baseline runs fall within the declared noise policy, every metric
in the budget table is collected, and the benchmark lane detects a seeded
runtime, compile-time, memory, and size regression.

### S6.2 — Explicit verified pass pipelines and invalidation

1. Introduce one pipeline description in `src/backend/pliron/` that maps the
   public profile to an ordered list of Pliron and LLVM passes. Suggested
   initial pipelines:
   - `O0`: Pliron mem2reg, DCE, verification; no LLVM optimization pipeline.
   - `release`: the verified Pliron cleanup pipeline followed by the pinned
     LLVM `default<O1>` pipeline. Evaluate `default<O2>` only as an isolated
     benchmark experiment and adopt it only if all budgets pass.
2. Verify after every correctness-sensitive Pliron phase in tests, and verify
   the LLVM module before and after external optimization. Reject residual
   illegal operations before emission.
3. Add pass instrumentation that records phase name, elapsed time, peak/ending
   IR size, and whether a pass changed the module. Keep it disabled unless
   measurement is requested.
4. Add focused unit tests for analysis preservation and invalidation. A
   mutating pass must invalidate cached analyses it cannot prove preserved; a
   non-mutating pass must not. Exercise nested function passes and module-level
   passes, since the current cleanup pipeline crosses that boundary.
5. Add optimization regression fixtures for overflow, traps, references,
   tagged outcomes, `finally`, destructor order, Variant active payloads,
   SIMD lane semantics, aliasing collections, and runtime calls. For each,
   compare VM, `O0`, and release observable behavior.
6. Snapshot the selected pipeline and LLVM tool invocation so a dependency
   upgrade cannot silently change optimization policy.

Exit: pipeline construction is centralized and deterministic; invalidation
tests fail under a deliberately stale analysis; every phase verifies; and the
full differential and sanitizer suites pass at both profiles.

### S6.3 — Source-level LLVM debug locations

1. Extend the existing source locator boundary rather than reading AST/HIR from
   the backend. Intern source files deterministically and translate MIR/Pliron
   locations to LLVM compile-unit, file, subprogram, and instruction locations.
2. Preserve inlining scope only when an actual inlining pass is enabled. Mark
   compiler-generated wrappers and cleanup blocks as artificial while retaining
   the nearest user source scope for backtraces.
3. Define path policy: debug builds use remapped workspace-relative paths by
   default; an explicit prefix-map option handles reproducible external build
   roots. Never embed temporary paths.
4. Test LLVM IR metadata structurally, then compile a small crashing/trapping
   fixture and inspect it with a pinned LLVM debugger/symbolizer to assert the
   source file and line. Test non-ASCII source and linked modules.
5. Keep debug information orthogonal to optimization and strip policy: `O0`
   defaults to debug information for developer builds; release artifacts may
   retain a separate debug file while the executable is stripped deterministically.

Exit: source-level backtraces name the correct file and line at `O0` and retain
useful call-site locations in release; debug metadata contains no undeclared
absolute or temporary paths.

### S6.4 — Deterministic object and executable linking

1. Replace ad hoc tool discovery with a resolved, reportable toolchain object:
   exact Clang/`opt` paths, versions, target, data layout, CPU features, profile,
   linker mode, runtime ABI version, and runtime archive digest. Fail before
   lowering when required tools are absent or incompatible.
2. Make all command lines deterministic and target-explicit. Control build IDs,
   timestamps, archive member metadata/order, debug-prefix maps, locale, and
   environment inputs. Sort every generated symbol/global/input list.
3. Write outputs through a temporary file in the destination directory and
   atomically rename only after successful verification/linking, preserving an
   existing output on failure.
4. For `.obj`, emit a sidecar link manifest containing schema version, target,
   ABI version, required libraries and order, toolchain constraints, and hashes.
   Provide a CLI link path that consumes and validates the manifest rather than
   asking users to reconstruct the Clang command.
5. Inspect emitted ELF files mechanically: target machine, sections, undefined
   symbols, exported `mj_*`/`mjrt_*` surface, executable stack/PIE policy,
   runtime ABI anchor, and absence of build-tree paths.
6. Add clean-build reproducibility tests for bitcode, object, unstripped
   executable, stripped executable, separate debug information, runtime archive,
   and object manifest. When bytes differ, print section-level and metadata
   diagnostics rather than a bare hash mismatch.

Exit: interrupted/failed builds do not corrupt outputs; two clean builds meet
the reproducibility contract; object manifests reject target, ABI, runtime hash,
or toolchain mismatches with an actionable diagnostic.

### S6.5 — Runtime and compiler distribution

1. Define a versioned release bundle for the supported Linux target, containing:
   - `bin/mojito` with the Pliron backend enabled;
   - the matching `lib/libmojito_runtime.a` and public link manifest metadata;
   - license/notices, toolchain compatibility, ABI version, target, and checksums;
   - optional separate debug symbols and a minimal compile/run smoke fixture.
2. Change runtime discovery to an explicit ordered contract: CLI/configured
   path, installation-relative bundle path, then development-only target-tree
   lookup. Record which archive was selected. Reject a wrong ABI, target, or
   digest before linking and name both expected and found values.
3. Ensure emitted executables statically include the required runtime and run in
   an empty environment/container without the compiler bundle, LLVM, Clang, or
   `MOJITO_RUNTIME_LIB` present. Inspect dynamic dependencies to prove this.
4. Make the release build create deterministic archives/checksums and an
   artifact manifest recording source revision, dependency lock digest,
   compiler/runtime versions, ABI, target, profile, toolchain versions, and
   reproducibility status.
5. Add installation-relocation tests, paths containing spaces and non-ASCII,
   missing/corrupt runtime tests, ABI mismatch tests, and a consumer test that
   links a saved object using only the bundle contract.

Exit: a clean release bundle compiles and links the smoke program after being
relocated; its executable runs in a compiler-free container; every incompatibility
fails early with remediation text; the bundle itself is reproducible.

### S6.6 — Acceptance, documentation, and promotion evidence

1. Run the complete VM/native differential corpus at `O0` and release, the
   sanitizer/lifecycle lanes, ABI cross-checks, symbol inspection, object-link
   consumer tests, clean-build reproducibility tests, and the full benchmark
   suite on the pinned runner.
2. Produce `docs/notes/pliron-stage6.md` with the final pass pipelines, tooling
   pins, debug/path policy, bundle format, raw-result references, budget table,
   waivers, nondeterministic sections, and remaining risks.
3. Update `docs/features.md`, `docs/architecture.md`, `docs/native-abi.md`,
   `docs/symbol-map.md`, CLI usage, and `docs/roadmap.md` only for behavior that
   actually landed. Keep Stage 6 completion separate from the Pliron promotion
   checkbox.
4. Rehearse one Pliron/LLVM dependency upgrade using the release and benchmark
   gates. Record required downstream patches, upstream submissions, elapsed
   engineering effort, output changes, and whether the fork/churn limits remain
   acceptable.
5. Hand the evidence packet to the promotion decision. If correctness,
   reproducibility, packaging, upgrade, or performance criteria fail, keep
   Pliron experimental and record whether to disable one optimization, repeat a
   bounded Stage 6 slice, or trigger the roadmap's Cranelift fallback.

Exit: every roadmap acceptance clause has a named automated test or a linked,
reviewed measurement artifact, with no unsupported claim inferred from a smoke
test.

## Expected Code and Test Ownership

| Area | Primary owner | Expected coverage |
|---|---|---|
| Profile and target configuration | `src/native/target.rs` | parse/validation tests; CLI tests |
| Pliron pipeline and instrumentation | `src/backend/pliron.rs` plus a focused `pipeline.rs` if needed | pass order, verification, invalidation, deterministic snapshots |
| LLVM optimization and tool resolution | `src/backend/pliron/emit.rs` | command snapshots, version mismatch, failure atomicity |
| Source/debug translation | existing locator in `src/backend/pliron/lower.rs`, emission adapter in `emit.rs` | metadata structure and symbolizer integration |
| Runtime bundle metadata and ABI validation | `src/native/rt_abi.rs`, `crates/mojito-runtime`, a focused distribution module | Rust/LLVM ABI cross-checks, corrupt/mismatch fixtures |
| CLI compile/link/package surface | CLI driver and `tests/cli_*` | installed/relocated bundle and object-consumer tests |
| Native differential/reproducibility | `tests/pliron_backend_test.rs` or split focused integration targets | `O0`/release parity, clean-build hashes, ELF inspection |
| Performance | `benchmarks/native/`, `scripts/bench-pliron` | pinned-runner results and quick smoke gate |

If these additions make `tests/pliron_backend_test.rs` or `emit.rs` materially
harder to navigate, split by ownership rather than adding more unrelated helper
sections. Preserve public-first item order in every Rust file.

## Required Gates

During implementation, each slice must pass its focused tests plus:

```text
env RUSTC_WRAPPER= scripts/check
scripts/check-pliron
git diff --check
```

Before Stage 6 is marked complete, also require the full `cargo nextest run`,
the pinned performance lane, clean-room bundle/relocation tests, compiler-free
execution tests, reproducibility rebuilds, and the dependency-upgrade rehearsal.

## Non-goals

- Changing Mojito source semantics, MIR schema, ownership, or drop policy.
- Making Pliron or LLVM part of the default VM build.
- Adding a second native backend during Stage 6.
- Supporting targets beyond the native ABI document's Linux target.
- Dynamic runtime loading, a general system linker, LTO/PGO, cross-compilation,
  native LLVM vector types, or full optimized variable debugging unless needed
  to pass a declared Stage 6 acceptance criterion.
- Marking Pliron preferred merely because Stage 6 implementation is complete;
  promotion remains a separate evidence-based roadmap decision.

## Completion Checklist

- [ ] Benchmark corpus, runner, schema, baseline, noise policy, and budgets are committed before optimization changes.
- [ ] `O0` and release pipelines are explicit, versioned, verified, and snapshot-tested.
- [ ] Analysis preservation/invalidation is tested for module and nested-function passes.
- [ ] Full differential, trap, sanitizer, ABI, and lifecycle suites pass at both profiles.
- [ ] Source file/line debug locations survive LLVM emission and produce useful backtraces.
- [ ] Object linking is manifest-driven and validates target/runtime ABI inputs.
- [ ] Executable and bundle creation is failure-atomic and deterministic.
- [ ] Clean builds meet the byte reproducibility contract or document narrowly isolated sections.
- [ ] Relocated bundles work; emitted executables run without the compiler or LLVM installed.
- [ ] ABI, target, toolchain, missing-runtime, and corrupt-runtime failures are actionable.
- [ ] Compile-time, peak-memory, IR/code-size, startup, and runtime budgets pass on the pinned runner.
- [ ] A dependency-upgrade rehearsal is recorded without a broad local fork.
- [ ] Architecture, ABI, feature, symbol, usage, stage-note, and roadmap documentation matches landed behavior.
- [ ] Required default, Pliron, full-suite, formatting, Clippy, and diff gates pass.
