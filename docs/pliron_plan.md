# Pliron Backend Evaluation and Adoption Plan

## Status

This is a future implementation plan, not approval to replace Mojito's current
backend architecture. It is based on the repository state and upstream Pliron
material reviewed on 2026-08-16.

## Decision

Adopt **an experimental Pliron backend below Mojito's verified MIR waist**, with
promotion to the preferred native backend only after it passes explicit semantic,
toolchain, portability, and performance gates.

This combines two strategies:

- Begin with an optional, experimental backend that cannot disturb the VM or the
  default build.
- Promote it to Mojito's primary native backend if the experiment succeeds.

Do not replace Mojito MIR with Pliron IR. MIR remains the serialized,
backend-independent compiler handoff and the VM remains the executable semantic
oracle. Pliron is a lowering and optimization framework below that handoff,
analogous in role—not identity—to MLIR in Mojo's native pipeline.

The first proof of concept should lower a deliberately small MIR subset directly
to Pliron's LLVM dialect. A narrow Mojito Pliron dialect should be introduced
only when a real semantic or optimization need cannot be represented cleanly in
the LLVM dialect. It must not become a second copy of all Mojito MIR.

## Why This Direction

Pliron provides an extensible Rust IR framework with operations, types,
attributes, regions, blocks, SSA values, parsing/printing, verification, pass
management, analyses, rewriting, dialect conversion, and an LLVM dialect. That
makes it a plausible Rust-native framework for expressing Mojito-specific
lowering stages while keeping the frontend and middle end free of C++ APIs.

The boundary must be described precisely:

- Pliron core is implemented in Rust and can support IR construction,
  verification, transformation, and textual inspection without making Mojito's
  default VM build depend on LLVM.
- Pliron's LLVM integration uses `llvm-sys` for conversion, bitcode, JIT, target,
  and native-code facilities. Native output therefore still requires a compatible
  LLVM installation and its platform toolchain.
- Pliron reduces API and ownership friction with C++ compiler frameworks; it does
  not remove LLVM from the native compilation toolchain.
- Upstream describes Pliron and its LLVM dialect as actively developed. Its API
  and supported LLVM surface must be treated as a versioned dependency, not as a
  stable platform promise.

The projects named by Pliron should be audited as evidence, not treated as proof
of production maturity. In particular, CubeCL's current public architecture
describes its own IR and multiple targets, so Stage 0 must identify whether and
where its current revision still depends on Pliron. The same exact-dependency
and code-path audit applies to CUDA Oxide.

## Preserved Mojito Contracts

The work must preserve these invariants throughout every stage:

```text
source
  -> lex / parse / link / comptime elaboration
  -> CheckedProgram
  -> HIR CFG
  -> MIR lowering and verification
  -> ownership analysis
  -> drop elaboration and post-elaboration verification
  -> VM or Pliron backend
```

- `Compiler` owns whole-program discovery, specialization, and lowering.
- `CheckedProgram` remains the semantic handoff into lowering.
- The backend accepts the same verified, drop-elaborated `MirProgram` that
  `CompiledProgram::emit_mir` serializes and `mojito exec` consumes.
- A backend does not inspect AST or HIR, repeat type checking, rediscover call
  binding, or implement ownership rules.
- MIR text remains the backend-independent artifact and reproducibility contract.
- VM execution remains the oracle until Pliron satisfies the final promotion
  gates. Unsupported native behavior must produce a contextual compile error,
  never silently fall back or change semantics.
- The default build, test suite, and `mojito run` behavior remain independent of
  Pliron and LLVM until a separate promotion decision changes that policy.

## Target Architecture

```text
                           +----------------------+
source -> Compiler -> executable, verified MIR --+--> VM (semantic oracle)
                           |                      |
                           |                      +--> canonical .mir artifact
                           v
                  MirToPliron lowering
                           |
             +-------------+----------------+
             |                              |
     Mojito Pliron ops                 LLVM dialect
     (only where needed)                    |
             +-------- dialect conversion --+
                                            |
                               verify / optimize / translate
                                            |
                              LLVM IR / bitcode / object / JIT
                                            |
                                      Mojito runtime ABI
```

### Backend boundary

Refactor the current execution-oriented backend API into two explicit roles:

1. `VmBackend::run(&MirProgram)` executes MIR and returns displayed bindings and
   captured output.
2. A native compilation interface accepts `&MirProgram`, `NativeTarget`, and
   `EmitKind`, returning a `NativeArtifact` or structured diagnostics.

The intended public shapes are:

```rust
pub enum EmitKind {
    PlironText,
    LlvmIr,
    LlvmBitcode,
    Object,
    Executable,
}

pub struct NativeTarget {
    pub triple: String,
    pub cpu: Option<String>,
    pub features: Vec<String>,
    pub optimization: OptimizationLevel,
}

pub enum NativeArtifact {
    Text(String),
    Bytes(Vec<u8>),
    Object(Vec<u8>),
    Executable(PathBuf),
}

pub trait NativeBackend {
    fn compile(
        &self,
        program: &MirProgram,
        target: &NativeTarget,
        emit: EmitKind,
    ) -> Result<NativeArtifact, Vec<Diagnostic>>;
}
```

The precise ownership of executable file creation may change during design
review, but the distinction between VM execution and native compilation must not.
`BackendKind::Pliron` remains unavailable unless the corresponding feature was
built. A request for an unavailable backend reports how to enable it.

### Command-line surface

Add commands incrementally rather than changing `run` during the experiment:

```text
mojito compile --backend pliron --emit pliron FILE
mojito compile --backend pliron --emit llvm-ir FILE
mojito compile --backend pliron --emit llvm-bc FILE -o FILE
mojito compile --backend pliron --emit object FILE -o FILE
mojito compile --backend pliron --emit executable FILE -o FILE
```

Text goes to stdout unless `-o` is supplied. Binary output requires `-o`.
`mojito run --backend pliron` is added only when the JIT or executable path can
match the VM's observable behavior for the enabled subset. Existing `emit-mir`
and `exec` commands remain unchanged.

### Pliron dialect policy

Use direct LLVM-dialect lowering for the Stage 1 scalar spike. Starting in Stage
2, add a `mojito` Pliron dialect only for operations that benefit from retaining
meaning above LLVM, initially:

- runtime calls whose ABI should not be duplicated in every lowering;
- checked arithmetic, traps, and language-level error propagation;
- string and aggregate constants before target layout is committed;
- lifecycle or cleanup operations that need validation before CFG expansion;
- source-location-bearing diagnostic sentinels during incomplete lowering.

Each such operation must have a verifier, textual syntax, negative tests, and a
conversion rule. Generic arithmetic, branches, calls, returns, loads, and stores
should use established Pliron/LLVM operations once their semantics are fixed.
There is no goal to reproduce the full MIR instruction set as dialect operations.

The lowering pipeline is:

```text
MirProgram
  -> construct Pliron module and symbol table
  -> Mojito/LLVM mixed IR verification
  -> normalize Mojito operations and explicit error/cleanup control flow
  -> dialect conversion to LLVM dialect
  -> LLVM-dialect verification
  -> optional optimization pipeline
  -> LLVM IR/bitcode
  -> LLVM target machine or linker
```

Every pass must declare its input and output invariants. Verification runs after
initial construction and after each conversion boundary in debug/test builds.

## Semantic and ABI Mapping

| Mojito concern | Pliron/native representation | Required decision or check |
|---|---|---|
| Specialized functions | One native symbol per specialized MIR function | Stable mangling includes module and specialization identity |
| Integers and booleans | Fixed LLVM integer types | Reject values whose checked MIR type has no selected width; preserve overflow policy |
| Floating point | Matching LLVM float type and ordered/unordered predicates | Pin NaN, signed-zero, and conversion behavior against VM tests |
| Control flow | Pliron blocks, SSA values, and LLVM branches | Preserve MIR edge arguments and terminator rules |
| Calls | Direct symbol calls first; indirect calls when MIR requires them | One ABI lowering owns argument/result layout |
| Strings | `{ptr, len}` value referencing UTF-8 bytes; constants in a module pool | Decide ownership bit/capacity representation before mutable strings |
| Aggregates | Target-layout-owned structs, never Rust `repr(Rust)` layouts | Add a shared layout module before aggregate execution |
| References | Native pointer plus any required runtime metadata | Ownership/origin validation is already complete; do not re-check it here |
| Destruction | Explicit calls and CFG edges already produced by drop elaboration | Differential tests must prove exactly-once and ordering behavior |
| Errors/exceptions | Explicit tagged outcome and CFG propagation initially | Do not use platform unwinding until it is separately specified and tested |
| `try`/`finally` | Explicit success/error edges through cleanup blocks | Every exit, including nested errors, must traverse required cleanup |
| Allocation | Versioned Mojito runtime ABI functions | Define alignment, zero-size, failure, and deallocation contracts |
| Printing | Runtime ABI operating on native value/string forms | Byte-for-byte stdout comparison with VM |
| Exact literals | Constants already selected by checked MIR plus constant pools | Backend must not parse source spellings or infer types |
| Source locations | Pliron location/debug metadata carried from MIR | Diagnostics retain module, span, and lowering stage |

Native exceptions should use explicit returns first, even if LLVM exception
handling becomes available. This is portable, matches MIR's explicit control-flow
model, and makes cleanup equivalence testable. Platform unwinding is a later
optimization proposal with its own compatibility gate.

### Runtime ABI

Create a small Rust runtime crate with a C ABI and an independently versioned
header/contract. It must not expose the VM's internal `Value` enum. Its first API
surface should cover process entry, captured/display output, string constants,
allocation/deallocation, and a tagged error result. Add aggregates, collections,
and reference helpers only as stages require them.

The ABI specification must define sizes, alignment, ownership, nullability,
failure behavior, symbol versioning, and who allocates/frees each value. Generate
or mechanically test declarations used by the Pliron lowering so the Rust runtime
and LLVM function types cannot drift unnoticed.

## Upstream Baseline and Pinning Policy

The upstream snapshots reviewed for this plan are not mutually synchronized:

- the current repository manifest reports workspace version `0.17.0`;
- the rendered API documentation reports `0.16.0`;
- the GitHub releases inspected include `v0.15.0`, while newer development is
  visible on the default branch;
- the current `pliron-llvm` manifest targets LLVM 22 through `llvm-sys` 221 and
  enables that integration by default.

Consequently, implementation must not use a floating Git branch or an imprecise
semver range. Stage 0 will select the newest published, mutually compatible
`pliron` and `pliron-llvm` release that passes the spike, pin both with exact
versions, retain `Cargo.lock`, and record:

- crate versions and source checksums;
- upstream Git commit and review date;
- supported Rust compiler and LLVM versions;
- enabled Cargo features;
- required system packages and environment discovery;
- local compatibility patches, if any, and their upstream issues.

If no published release passes, the experiment may use one immutable Git commit,
but promotion is blocked until Mojito either returns to a released version or
owns a documented update/fork policy.

## Capability and Fit Matrix

Stage 0 must replace every “verify” cell with a link to an upstream API, test, or
Mojito spike. Absence of evidence is a gap, not presumed support.

| Capability | Current evidence | Mojito requirement | Gate |
|---|---|---|---|
| Extensible ops/types/attributes | Core framework and declarative macros exist | Mojito/runtime ops with verification and text form | Build one op of each required category |
| Regions, blocks, SSA | Core IR supports them; dominance verification exists | MIR CFG and block-argument lowering | Diamond and loop verifier tests |
| Parse/print | Framework supports textual IR infrastructure | Stable diagnostic/debug snapshots | Byte-stable round trip for Mojito spike IR |
| Diagnostics/locations | Location and debug facilities exist | MIR provenance in every lowering error | Golden multi-module diagnostic |
| Pass management/analysis | Pass pipelines and cached analyses exist | Conversion and optimization with invalidation | Mutation/invalidation regression test |
| Rewriting/conversion | Rewrite and dialect conversion APIs exist and are evolving | Mojito-to-LLVM legality conversion | Full-conversion test rejects residual ops |
| LLVM dialect | Broad but upstream-described incomplete support | Every operation and type emitted by Mojito stages | Per-op inventory with no untracked fallback |
| LLVM export/JIT/targets | `pliron-llvm` wraps LLVM facilities | IR, bitcode, object, executable, optional JIT | Scalar program executes on supported host |
| Data layout | LLVM facilities exist; Pliron coverage must be verified | Deterministic aggregate/ABI layout | Cross-check against LLVM target data |
| Optimization | Core passes plus LLVM pipeline access | Correctness-preserving useful optimization | Differential tests at `O0` and optimized level |
| no-LLVM workflows | Core manifest separates Rust framework concerns | VM/default CI cannot require LLVM | Clean default build without LLVM installed |
| Real-project adoption | Upstream names CUDA Oxide and CubeCL | Evidence of maintained nontrivial usage | Exact dependency and used-API audit |

## Phased Implementation

No stage begins until the previous stage's exit gate passes. Each stage lands as
a reviewable change with an explicit rollback: remove its optional feature and
backend modules without changing MIR, VM, or source-language behavior.

### Stage 0 — Feasibility, dependency pin, and design record

Deliverables:

- Write `docs/pliron-compat.md` containing the immutable dependency baseline,
  LLVM/toolchain installation matrix, upstream API inventory, and tracked gaps.
- Audit Pliron's tutorial/Kaleidoscope implementation and the exact current
  Pliron use in CUDA Oxide and CubeCL: revision, dependency version, used APIs,
  target, test coverage, and evidence of active maintenance.
- Build minimal standalone examples for a custom operation, parser/printer,
  verifier failure, rewrite, conversion, LLVM module export, object emission, and
  host execution.
- Add a short architecture decision record confirming Pliron remains below MIR.
- Prototype the Cargo feature split without connecting it to `Compiler`.

Acceptance:

- A clean VM build succeeds on a machine with neither LLVM nor Pliron tooling.
- A pinned Pliron build succeeds in reproducible Linux CI with LLVM 22.
- The example emits and executes `main -> i32`, and its invalid variant produces
  a source-associated diagnostic rather than a panic.
- Every required API is classified supported, locally bridgeable, upstream gap,
  or blocker, with evidence.

Stop if LLVM installation cannot be isolated from the default build, required
APIs require a broad Mojito-maintained fork, or the upstream update cadence makes
the pin operationally untenable.

### Stage 1 — Scalar IR and end-to-end native spike

Scope: integer/boolean constants and arithmetic, comparisons, unconditional and
conditional branches, direct calls, and returns. Lower directly to the LLVM
dialect; do not add a Mojito dialect yet.

Deliverables:

- Add optional `pliron-backend` and `pliron-llvm` integration crates/modules.
- Implement `MirToPliron` with total pattern matching over the supported subset
  and one structured unsupported-instruction diagnostic.
- Implement Pliron text, LLVM IR/bitcode, object, and host-executable outputs.
- Add deterministic symbol mangling and source-location propagation.
- Add CLI `compile --backend pliron` behind the feature.

Acceptance:

- Representative straight-line, diamond, loop, multi-function, and recursive
  scalar programs match VM exit result and observable output.
- Pliron text round-trips byte-stably after canonical printing.
- Invalid constructed IR fails verification; unsupported valid MIR fails with
  function, block, instruction, and source location.
- Repeated builds of the same input and target produce byte-identical LLVM IR;
  explain or eliminate differences in binary artifacts.
- No AST, HIR, checker, ownership, or call-binding type is imported by the
  backend module.

### Stage 2 — Executable scalar language subset

Scope: all currently supported scalar operators and conversions, local mutable
storage where needed, function parameters/results, recursion, and supported
control-flow constructs.

Deliverables:

- Introduce the Mojito Pliron dialect only for checked traps, runtime calls, or
  another demonstrated semantic operation that LLVM dialect cannot express at
  the desired level.
- Add a legality-driven full conversion; LLVM emission fails if any Mojito op is
  left behind.
- Define optimization levels and a conservative initial pass pipeline.
- Add optional host JIT execution for differential tests; do not make it the
  public `run` default.

Acceptance:

- All scalar `run` conformance rows supported by the native capability manifest
  match VM stdout, result, trap category, and displayed bindings where relevant.
- Both `O0` and the initial optimized pipeline pass the differential suite.
- A guard test fails if the eligible test count unexpectedly decreases.
- The backend reports every excluded MIR instruction/type in a generated
  capability inventory.

### Stage 3 — Layout, runtime ABI, strings, aggregates, and errors

Scope: native data layout, printing, strings, tuples/struct-like aggregates,
allocation, runtime failures, and error values. Collections are not included
unless their layout is ready.

Deliverables:

- Add the shared native layout/ABI module and versioned Rust runtime crate.
- Lower string and aggregate constants through Mojito dialect operations before
  layout conversion.
- Implement runtime calls for output, allocation, deallocation, and error
  reporting.
- Add ABI conformance tests that compare Rust-side and LLVM-side size/alignment
  and function signatures.

Acceptance:

- String, aggregate, allocation, and runtime-error fixtures match VM output and
  failure classification.
- Sanitizer-enabled native tests show no leak, double free, misalignment, or
  use-after-free in the stage corpus.
- Runtime symbols and ABI version are inspected in produced objects.
- Cross-compiling layout-only tests for every supported target produces expected
  sizes and alignments without executing target code.

### Stage 4 — Ownership effects, destruction, and exceptional control flow

Scope: all currently supported drop behavior, references, error propagation,
`try`, and `finally`.

Deliverables:

- Consume drop-elaborated MIR exactly as emitted; document why no ownership
  analysis occurs in Pliron.
- Normalize success/error outcomes to explicit tagged values and CFG edges.
- Lower every cleanup exit explicitly, including return, error, and nested
  control flow. Do not introduce platform unwinding.
- Add lifecycle instrumentation in the test runtime for ordered event comparison.

Acceptance:

- All ownership and destruction corpus cases eligible for execution have the
  same ordered create/drop/error trace as the VM.
- Negative ownership programs still fail before reaching the backend.
- Reference and interior-origin tests preserve value behavior and trap at the
  same semantic boundary as the VM.
- Nested `try`/`finally`, errors raised in cleanup, and early returns have
  dedicated differential tests.

### Stage 5 — Language parity for the supported Mojito subset

Scope: specialized generics, retained callable forms, collections, rich
literals, references, and SIMD/features that require additional LLVM coverage.

Deliverables:

- Work from a generated MIR instruction/type/runtime capability matrix.
- Add features one vertical slice at a time: layout, dialect operation if needed,
  conversion, runtime support, diagnostics, and differential tests together.
- Open upstream issues or narrowly scoped patches for missing Pliron LLVM ops;
  track every patch in the compatibility document.

Acceptance:

- Every runnable conformance case is either native-equivalent or explicitly
  excluded by a reviewed backend limitation; promotion requires zero exclusions
  for Mojito's advertised executable subset.
- Canonical `.mir` artifacts execute identically through VM and native paths.
- The native backend accepts no source program that the normal compiler rejects.

### Stage 6 — Optimization and distributable native artifacts

Scope: stable object/executable output, runtime linking, debug information,
optimization, build reproducibility, and supported target packaging.

Deliverables:

- Define `O0` and release pipelines, pass ordering, verification points, and
  analysis invalidation.
- Produce relocatable objects and linked executables with explicit target and
  linker configuration.
- Add debug locations sufficient to map native failures to Mojito source.
- Establish compile-time, peak-memory, code-size, startup, and runtime benchmarks.

Acceptance:

- Optimized and unoptimized outputs pass the complete differential suite.
- On the agreed benchmark set, native execution demonstrates a material benefit
  over the VM without unacceptable compile-time or binary-size regression. Set
  numeric thresholds in the Stage 6 design review before collecting results.
- Two clean builds in the pinned container produce reproducible objects, or all
  remaining nondeterministic sections are documented and excluded deliberately.
- Produced executables run without the Mojito compiler present and report runtime
  ABI mismatches clearly.

### Stage 7 — Promotion decision

Promotion changes Pliron from experimental to Mojito's preferred native backend;
it does not remove the VM or make native compilation the only execution path.

Required gates:

- semantic parity for all runnable conformance and corpus cases;
- zero untracked unsupported MIR operations or types;
- Linux support and documented, tested status for macOS and Windows;
- a reproducible dependency/toolchain installation story;
- no Mojito-maintained broad fork of Pliron;
- acceptable compile-time, memory, code-size, and runtime benchmark results;
- at least one successful upgrade rehearsal to a newer Pliron release;
- six weeks of CI without unresolved backend correctness regressions;
- architecture, features, symbol map, README, roadmap, and release documentation
  updated in the same promotion change.

If any gate fails, keep Pliron experimental or remove the optional backend. Since
MIR and VM remain intact, rollback does not affect language semantics or textual
artifacts.

## Testing and Observability

Maintain four complementary test layers:

1. **IR unit tests:** one positive and negative test per lowering, verifier,
   rewrite, conversion, ABI type, and unsupported diagnostic.
2. **Snapshot tests:** canonical Pliron and LLVM text for a small stable set;
   normalize line endings and require one trailing newline.
3. **Differential tests:** compile the same `MirProgram` to VM and native forms;
   compare stdout bytes, displayed bindings, return/error category, and ordered
   lifecycle events. Never compare only process exit codes.
4. **Artifact tests:** `.mir -> VM` and `.mir -> Pliron -> native` consume the
   same artifact; direct source compilation must be equivalent to both.

Every stage records compile duration, peak RSS, IR size at each boundary, object
size, execution time, and the number of supported/excluded MIR operations and
conformance cases. Store benchmark methodology and raw machine metadata. Treat
performance results as descriptive until Stage 6 fixes thresholds in advance.

CI lanes should be:

- default VM lane on all supported platforms, with no LLVM installation;
- Pliron core/text lane where supported, without native LLVM conversion;
- pinned LLVM 22 native lane on Linux initially;
- macOS native lane once toolchain discovery and linking are reproducible;
- Windows compile/layout lane first, promoted to execution only after upstream
  LLVM and linker behavior is proven.

Nightly or scheduled CI should exercise sanitizers, optimized differential tests,
reproducibility, benchmarks, and dependency-update rehearsal. Pull-request CI
should keep focused scalar/native tests reasonably fast.

## Dependency and Distribution Policy

- Keep Pliron dependencies optional and outside default features during Stages
  0–6.
- Separate core Pliron/text support from `pliron-llvm`/`llvm-sys` native support
  where Cargo's feature graph permits it. Confirm actual behavior with
  `cargo tree -e features`; do not rely on package names as proof of isolation.
- Pin LLVM major/minor to the version required by the chosen `pliron-llvm` pin.
- Publish tested installation instructions per OS and fail configuration with an
  actionable message that reports discovered LLVM version and search locations.
- Cache or containerize CI tooling, but also test the documented clean-machine
  installation path.
- Keep the runtime library small, versioned, target-specific, and distributable
  independently of the compiler executable.

Linux is the first supported native host/target. macOS follows after object
emission, linker, and runtime packaging pass. Windows remains experimental until
the same gates pass; it must not be advertised merely because LLVM can target it.
Cross-compilation support is a separate feature from host execution.

## Alternatives

| Option | Advantages | Costs and risks | Decision |
|---|---|---|---|
| Replace MIR with Pliron | One fewer named IR; all passes use one framework | Breaks stable artifact and VM boundary; couples frontend semantics to evolving dependency; large migration | Reject |
| Pliron directly to LLVM dialect | Smallest spike; quickly tests export and toolchain | LLVM-level IR loses useful language/runtime structure early | Use for Stage 1 only |
| Narrow Mojito dialect then LLVM conversion | Retains useful semantics for verification and lowering; supports progressive conversion | More operations, parsers, verifiers, and maintenance | Adopt only for demonstrated needs |
| Inkwell directly over LLVM | Mature LLVM access from Rust; fewer IR layers | LLVM-centric API and lifetime friction; no MLIR-like dialect/conversion framework | Retain as fallback comparison |
| Melior/MLIR | Closest ecosystem to Mojo's stated MLIR use; broad dialect tooling | C/C++ MLIR build and version friction remains; Rust wrapper coverage/version coupling | Revisit if Pliron gaps block parity |
| Handwritten LLVM IR/bitcode | Maximum dependency control at a narrow surface | High correctness and maintenance burden; recreates verifier/layout/tooling work | Reject as primary plan |
| Keep VM only | Lowest immediate complexity | No native-code path | Baseline, not end state |

At the end of Stage 1, implement the same representative function with the
minimum viable Inkwell and Melior paths in a throwaway benchmark branch. Compare
dependency setup, Rust-side code complexity, diagnostics, IR fidelity, compile
time, and artifact execution. This prevents selecting Pliron on language or API
preference alone. Those comparison spikes do not enter production.

## Risk Register

| Risk | Early signal | Mitigation | Stop/promotion effect |
|---|---|---|---|
| Pliron API churn | Frequent breaking changes or expensive upgrade rehearsal | Exact pin, compatibility layer, upstream engagement | Blocks promotion if maintenance is disproportionate |
| LLVM dialect gaps | Required MIR operation lacks valid conversion/export | Inventory per vertical slice; contribute narrow upstream support | Keep experimental or reconsider Melior/Inkwell |
| LLVM remains difficult to distribute | LLVM discovery/linking fails across clean hosts | Optional features, pinned containers, per-OS docs | Default remains VM; blocks advertised platform |
| Semantic drift from VM | Differential output/error/drop mismatch | MIR-only input, explicit ABI, oracle tests at `O0` and optimized levels | Correctness failure blocks stage exit |
| Duplicate IR maintenance | Mojito dialect mirrors MIR mechanically | Dialect admission rule and review checklist | Delete redundant ops before advancing |
| Data-layout mismatch | Aggregate sizes differ across Rust/LLVM/targets | One layout owner and ABI cross-checks | Blocks aggregates and native promotion |
| Exception/cleanup mismatch | Drops skipped or reordered on error paths | Explicit outcome CFG; lifecycle event traces | Blocks Stage 4 |
| Misleading “pure Rust” claim | Users still need LLVM/system packages | Document core/native split accurately | Documentation release blocker |
| Project-adoption evidence is stale | Named projects no longer depend on Pliron | Exact revision/dependency audit | Reduces confidence; not alone a blocker |
| Optimization miscompile | `O0` passes while release differs | Differential suite at every optimization level; pass bisection | Disable offending pass; unresolved issue blocks promotion |
| Runtime ABI lock-in | Layout changes require incompatible artifacts | ABI version symbol and explicit compatibility check | Requires migration plan before stability promise |
| Bus factor/upstream discontinuity | Issues/PRs stall; releases cease | Minimize API surface, document fork cost, retain alternatives | Trigger reassessment and possible rollback |

## Smallest Useful Proof of Concept

The first executable slice is one source program that specializes two functions,
performs integer arithmetic and comparison, takes both sides of a conditional in
separate test cases, calls the second function, and returns an integer. It must:

1. compile normally to the cached, verified, drop-elaborated `MirProgram`;
2. execute in the VM;
3. lower to Pliron's LLVM dialect without reading AST, HIR, or `CheckedProgram`;
4. print and reparse canonical Pliron text;
5. verify before and after conversion;
6. emit LLVM IR, bitcode, an object, and a host executable;
7. execute with the same result as the VM;
8. turn one deliberately unsupported MIR instruction into a source-associated
   diagnostic; and
9. build behind an optional feature while the default build succeeds without
   LLVM installed.

Passing this proof only authorizes Stage 2. It does not establish that Pliron is
the primary backend.

## Documentation and Task Lifecycle

At every implemented stage:

- update `docs/features.md` with supported native capabilities and explicit gaps;
- update `docs/symbol-map.md` when backend, dialect, layout, or runtime ownership
  moves;
- update `docs/architecture.md` for new lasting boundaries;
- update README build requirements and CLI only for commands that exist;
- remove the completed roadmap subtask rather than retaining checked entries;
- update `commit_msg.txt` with that completed task's commit message; and
- run formatting, focused and full relevant tests, Clippy with warnings denied,
  `git diff --check`, and `env RUSTC_WRAPPER= scripts/check` before reporting the
  stage complete.

Do not add future Pliron capabilities to the advertised feature matrix before
they meet their stage acceptance criteria.

## Research Sources

- [Pliron repository and project overview](https://github.com/pliron-org/pliron)
- [Pliron workspace manifest](https://raw.githubusercontent.com/pliron-org/pliron/master/Cargo.toml)
- [Pliron LLVM dialect and integration](https://github.com/pliron-org/pliron/tree/master/pliron-llvm)
- [Pliron LLVM integration manifest](https://raw.githubusercontent.com/pliron-org/pliron/master/pliron-llvm/Cargo.toml)
- [Pliron pass infrastructure documentation](https://pliron-org.github.io/pliron/pliron/pass/index.html)
- [Pliron releases and API evolution](https://github.com/pliron-org/pliron/releases)
- [CUDA Oxide repository](https://github.com/NVlabs/cuda-oxide)
- [CubeCL repository and current architecture](https://github.com/tracel-ai/cubecl)
