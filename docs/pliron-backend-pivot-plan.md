# Pliron as Mojito's Required Compiler IR Framework

## Decision

**Conditional GO for option A, implemented as a gated migration rather than a
rewrite.** Pliron is capable enough to become Mojito's required IR framework
below `CheckedProgram` for Mojito's current, CPU-first scope. It is not an MLIR
replacement in ecosystem breadth, optimizer inventory, target coverage, tooling,
or organizational maturity, and it cannot give Mojito Mojo's heterogeneous
compiler stack merely by occupying the same architectural position.

The target architecture is:

```text
source -> parse/link/elaborate/check -> CheckedProgram
                                      |
                                      v
                         mojito.semantic Pliron dialect
                                      |
                    ownership, liveness, drop elaboration
                                      |
                                      v
                           mojito.core Pliron dialect
                          /            |             \
                 register VM     optimization     inspection
                                      |
                                      v
                           mojito.abi Pliron dialect
                                      |
                                      v
                             Pliron LLVM dialect
                                      |
                                      v
                         LLVM IR / object / executable
```

`CheckedProgram` remains the language-semantic handoff. Pliron becomes the
required representation framework below that boundary. The register VM remains
the executable oracle, but it eventually executes verified `mojito.core` rather
than a parallel Rust MIR object model. Ownership and drop elaboration remain
Mojito passes; moving their IR storage into Pliron does not move their language
policy into Pliron.

The current textual `.mir` contract is preserved through migration and then
versioned into the canonical textual form of `mojito.core`. Existing artifacts
continue to load through a compatibility parser. This is deliberately not
Mojo's reported parser-direct-to-MLIR frontend: Mojito retains its AST,
`CheckedProgram`, and explicit semantic checks because they are useful
correctness boundaries for this project.

## What “like Mojo” does and does not mean

Public Modular material says Mojo is built around MLIR, lowers through layered
dialects, exposes MLIR types/attributes/operations/regions to source programs,
and uses LLVM-level and other MLIR code-generation backends. It also reports a
parser-direct-to-MLIR architecture. Those facts establish MLIR as both Mojo's
compiler substrate and an exposed language extension surface.

For Mojito, tight coupling should mean:

- all post-check IR is Pliron-owned operations, regions, blocks, SSA values,
  attributes, types, symbols, locations, and interfaces;
- Mojito optimizations and analyses are Pliron passes with normal invalidation;
- language-specific levels are explicit dialects with total verified lowering;
- the VM, native lowering, printer, inspector, and future backends consume the
  same Pliron IR rather than parallel representations;
- native LLVM translation is only the final target-specific portion.

It should not initially mean:

- exposing arbitrary Pliron syntax in Mojo source;
- removing `CheckedProgram`, the VM, or source-level semantic checks;
- depending on LLVM for checking, VM execution, `.mir` parsing, or IR tests;
- claiming MLIR's GPU/accelerator ecosystem, transform dialect, bufferization,
  affine/polyhedral stack, vector lowering, or target breadth;
- reproducing every MLIR abstraction before Mojito has a concrete consumer.

## Evidence and evaluated pins

The implementation baseline is the repository's exact crates.io pin:

- `pliron = 0.17.0`, released 2026-08-07;
- `pliron-llvm = 0.17.0`;
- `llvm-sys = 221.0.1`, requiring LLVM 22 (locally 22.1.8);
- Rust 1.96.1; Apache-2.0 licensing.

The 2026-08-18 Stage 0 audit records 609 upstream commits, active maintenance,
frequent breaking 0.x releases, a strong single-maintainer concentration, no
changelog, and incomplete generated documentation for `pliron-llvm`. Before the
first migration stage, refresh this pin record against upstream head and run a
0.17-to-current upgrade rehearsal. Do not begin the cutover on an unevaluated
moving branch.

Current evidence is materially stronger than a toy demonstration:

- Pliron provides extensible operations, types, attributes, interfaces,
  canonical parsing/printing, regions, blocks, block arguments, SSA def-use,
  verification, diagnostics/locations, rewrite infrastructure, dialect
  conversion, dominance/liveness, mem2reg, DCE, pass composition, and analysis
  invalidation.
- `pliron-llvm` provides an incomplete but practical LLVM dialect, conversion
  to LLVM IR, bitcode export, and ORC LLJIT integration. Its own `llvm-opt`
  tests compile bzip2 and Lua.
- Mojito already lowers substantial verified MIR directly to the LLVM dialect,
  runs mem2reg/DCE, emits LLVM IR/bitcode/objects/executables, and differentially
  checks semantics against the VM.
- Pliron's Kaleidoscope example is useful tutorial evidence, not a maturity
  argument by itself.
- CUDA Oxide is the most relevant external architecture: Rust MIR lowers to a
  custom Pliron MIR dialect, then to LLVM/NVVM and PTX. It is active but labels
  itself experimental/alpha; it validates architectural feasibility, not
  production stability.
- CubeCL is a production-oriented multi-backend project, but its present public
  architecture is not evidence that its main compiler is centered on Pliron.
  `pliron-spirv` and contributor overlap show ecosystem interest only.

## Pliron versus MLIR

| Dimension | Pliron 0.17 | MLIR | Consequence for Mojito |
|---|---|---|---|
| Core IR | Rust-native operations, attributes, types, regions, blocks, SSA, interfaces | Mature C++ implementation of the same broad extensible model | Pliron is sufficient for Mojito's post-check IR shape. |
| Definitions | Rust proc macros and declarative definitions | ODS/TableGen, generated builders/verifiers/docs, declarative rewrites | Pliron is pleasant for Rust integration but has less generation and tooling depth. |
| Conversion | Pattern/rewrite and dialect conversion exist | Full/partial/analysis conversion, legality targets, type conversion and materialization | Mojito must add a strict legality layer and likely upstream it. |
| Passes/analysis | Composable passes, cached analyses, invalidation, dominance and liveness | Large mature pass/analysis ecosystem, instrumentation, threading, reproducer tooling | Framework fit is good; ready-made optimization leverage is much smaller. |
| Dialects | Core/builtin, LLVM, and a small external ecosystem | Extensive standard and target dialect catalog | Mojito must own `mojito.*` and most progressive lowering levels. |
| Layout/target modeling | No MLIR-equivalent general data-layout framework in the audited release | DLTI and type/op/dialect data-layout interfaces | Keep `native::layout` authoritative initially; upstream only generic hooks with proven need. |
| Diagnostics/tools | Located errors and canonical text; limited surrounding tools | Mature diagnostics, `mlir-opt`, bytecode, LSP/editor support, pass pipelines and reproducers | Mojito must build inspection, pass-pipeline, and reducer quality itself. |
| Native code | Through `pliron-llvm` and native LLVM libraries | Through LLVM dialect/export and the LLVM stack | Neither is a pure-Rust machine-code toolchain. |
| Stability | Active 0.x project, breaking monthly minors, concentrated maintenance | Large LLVM project with scheduled releases and broad maintainership | Required use demands an adapter boundary, exact pins, upgrade rehearsals, and fork budget. |
| Heterogeneous scope | Promising custom-dialect substrate; limited standard target stack | Proven CPU/GPU/accelerator compiler ecosystem | Pliron fits Mojito's current CPU scope, not Mojo's full hardware ambition today. |

The central distinction is important: `pliron` core is a Rust IR framework and
can remain usable in VM-only builds without LLVM. `pliron-llvm` depends on
`llvm-sys` and native LLVM 22. It reduces C++ API/binding friction; it does not
remove LLVM libraries, LLVM version coupling, platform packaging, or link-time
cost. Runnable native code still requires installed or distributed LLVM
components (or an external tool such as Clang for the current executable path).

## Comparison with direct LLVM and Melior

| Choice | Engineering friction | Extensibility/middle end | Native dependency | Maturity | Long-term fit |
|---|---|---|---|---|---|
| Pliron + `pliron-llvm` | Idiomatic Rust for IR and passes; LLVM isolated late | Strong framework, small ecosystem; Mojito owns most dialects/passes | LLVM still required for native output | Young, concentrated maintenance | Best fit if Mojito accepts framework ownership and contributes upstream. |
| MLIR via Melior | Rust wrapper plus MLIR C API, CMake/package/version friction | Full MLIR concepts and dialect ecosystem where exposed | MLIR and LLVM native libraries | MLIR mature; binding surface adds its own churn | Best if accelerator ecosystem becomes more important than Rust-native development. |
| LLVM via Inkwell | Direct and comparatively simple CPU code generation | No natural multi-level language IR; custom middle end remains Mojito's | LLVM native libraries | LLVM mature; binding/version coupling remains | Best shortest path to CPU code, poor match for the requested architectural role. |
| Cranelift | Excellent Rust integration and straightforward machine code | Backend, not a multi-level extensible IR framework | No LLVM | Mature CPU backend | Strong fallback for dependable CPU codegen, not a substitute for MLIR's role. |

## Capability fit

| Mojito family | Pliron representation | Required Mojito work |
|---|---|---|
| Functions/modules/symbols | module/function-like ops, regions, symbol attributes | Define visibility, linkage, declaration/body and symbol-table invariants. |
| CFG/terminators | blocks, block arguments, successors, branch/return ops | Normalize structured `Try`; verify successor signatures and cleanup state. |
| Registers/SSA | SSA results and block arguments | Eliminate register-number maps; preserve stable debug IDs only as attributes. |
| Variables/places | explicit slot/address ops plus typed projection ops | Keep place identity until ownership/drop passes finish; lower later to addresses/GEPs. |
| Scalar/literal ops | `mojito.semantic` exact ops, then builtin/LLVM scalar ops | Preserve exact literal and wrapping/trapping semantics before target lowering. |
| Struct/tuple/variant | nominal Mojito types and aggregate ops | Keep nominal identity and lifecycle interfaces; lower layout through `mojito.abi`. |
| Calls/generics | symbol calls plus explicit application attributes | Backend monomorphization becomes a Pliron pass; call binding remains pre-IR policy. |
| Methods/traits | checked exact symbol plus receiver metadata | No dynamic language re-resolution; devirtualization/retargeting is explicit conversion. |
| References/pointers | distinct checked reference/place types, later ABI pointers | Origins remain analysis attributes until verification; erase only at ABI lowering. |
| Ownership/moves | use/copy/move/consume/drop ops and ownership interfaces | Port analyses and drop elaboration; do not encode these initially as LLVM memory ops. |
| Exceptions/`try` | structured regions in semantic dialect | Lower to explicit outcome CFG before core/ABI legality; preserve `finally` precedence. |
| Closures/callables | environment types, closure construction and indirect-call ops | Closure conversion and thunk generation in Mojito passes. |
| Iteration/collections | semantic protocol/call/subscript ops | Specialize/devirtualize, then lower through ordinary calls/runtime operations. |
| SIMD | target-independent shaped/vector type and semantic ops | Scalar fallback first; vector lowering needs a Mojito dialect or upstream vector support. |
| Runtime ABI | `mojito.abi` calls, outcomes, strings and allocation types | Keep `native::{layout,rt_abi,target,mangle}` as normative Rust policy exposed to passes. |
| Debug/source provenance | Pliron locations plus stable source/node attributes | Define lossless round-trip rules and test every conversion. |

## Dialect design

### `mojito.semantic`

This is the first post-`CheckedProgram` IR and the owner of language-shaped
facts still needed by ownership and specialization. It contains nominal types,
exact literals, places and projections, checked direct/method/subscript calls,
structured `try`, closures, reference origins, and explicit use conventions.
It is not executable until verified.

### `mojito.core`

This is the new stable executable waist. It is monomorphic, CFG-normalized,
ownership-verified, and drop-elaborated. It retains explicit lifecycle,
reference, checked trap, outcome, aggregate, and runtime operations where LLVM
would erase too much meaning. The VM executes this dialect. Canonical
`mojito.core` text becomes `.mir` schema version 2.

### `mojito.abi`

This short-lived lowering dialect makes target layout, aggregate passing,
raising outcomes, runtime calls, wrapper generation, and mangled symbols
explicit. It is the only layer allowed to depend on `native::target`, layout,
and runtime ABI decisions. It then converts totally to Pliron LLVM dialect.

Avoid a dialect per frontend phase. Add a level only when it gives a verifier,
analysis, optimization, or multiple lowering paths a stable contract.

## Staged migration

### Stage A0 — refresh and harden Pliron

- Re-audit current upstream and pin one release/revision.
- Add an internal `ir_framework` adapter module so proc macros, context/pointer
  types, pass APIs, and diagnostic conversion do not leak through all compiler
  modules.
- Prototype strict conversion legality, type conversion, pass-pipeline parsing,
  canonical round trips, and crash-free invalid-IR diagnostics.
- Upstream legality/round-trip fixes where generally useful.
- Exit: one upgrade rehearsal, no broad fork, Linux LLVM and core-only gates
  reproducible, and a written two-person-or-bus-factor maintenance response.
- Roll back to option B if this requires a broad fork or upstream churn consumes
  more maintenance than the native backend itself.

### Stage A1 — shadow `mojito.core` dialect

- Define types, attributes, locations, function/module, scalar, CFG, call,
  variable/place, and lifecycle operations with verifiers and textual syntax.
- Translate current verified MIR to `mojito.core` and back in tests only.
- Run byte/semantic differential checks: old MIR → VM versus MIR → core → MIR → VM.
- Measure construction time, verification time, memory, and text size.
- Exit: all canonical MIR fixtures round-trip semantically, every malformed op
  fails diagnostically, and overhead stays under an agreed budget (initially
  20% compile time and 30% peak memory on the corpus).
- This stage lands with Pliron optional.

### Stage A2 — VM consumes Pliron core

- Add a core interpreter adapter using stable op interfaces, not downcasts
  scattered through the VM.
- Execute scalar, CFG, call, aggregate, reference, outcome, and lifecycle
  families incrementally; dual-run against the existing VM after each family.
- Preserve deterministic ASAP destruction and event ordering exactly.
- Exit: the full executable corpus has identical output/errors/lifecycle under
  old-MIR and Pliron-core VM paths, including loaded textual artifacts.
- Pliron remains optional until the dual path is complete.

### Stage A3 — port ownership and drop elaboration

- Emit `mojito.semantic` from `CheckedProgram` in shadow mode.
- Port ownership, liveness, interior-origin, initialization, and drop
  elaboration into Pliron analyses/passes with explicit preservation rules.
- Compare analysis facts and final core text against the current pipeline.
- Exit: positive/negative ownership corpus and source locations are identical;
  randomized differential generation finds no mismatch; no semantic rule is in
  a generic Pliron verifier or LLVM lowering.

### Stage A4 — make Pliron core the stable waist

- Switch `Compiler` production output to verified `mojito.core`.
- Make `.mir` v2 canonical Pliron text; retain a v1 reader translating into
  core, and freeze a compatibility corpus.
- Remove the Rust MIR execution path only after one release cycle; keep its
  schema reader until the artifact support policy permits removal.
- At this point core `pliron` becomes a required dependency, but
  `pliron-llvm`, `llvm-sys`, and LLVM remain optional.
- Exit: default VM build contains no LLVM dependencies, corpus parity is exact,
  artifact compatibility is documented, and rollback remains possible by
  reverting the producer switch rather than reconstructing deleted semantics.

### Stage A5 — introduce `mojito.abi` and total lowering

- Refactor the existing direct LLVM lowering behind core-to-ABI and ABI-to-LLVM
  conversions.
- Establish explicit legality sets and fail if any semantic/core/ABI op remains
  before LLVM export.
- Move backend monomorphization, closure conversion, exception CFG expansion,
  layout, calling convention, wrapper, and runtime-call materialization into
  ordered passes at the highest level that retains the required facts.
- Exit: current native parity and sanitizer gates pass at O0/O1; direct MIR-to-
  LLVM lowering can be deleted; non-LLVM backends can consume core or ABI
  without importing LLVM concepts.

### Stage A6 — optimization framework

- Add canonicalization, SCCP/constant folding, CFG simplification, inlining,
  devirtualization, dead code, escape-based allocation simplification, and
  ownership-aware move/drop optimization at core level.
- Keep LLVM optimization for target-specific cleanup and code generation.
- Every optimization receives a verifier boundary, pass-pipeline spelling,
  differential test, and disable switch.
- Exit: optimized output remains exact, native performance improves materially,
  compile-time/memory budgets hold, and a pass pipeline can be reproduced from
  a bug report.

### Stage A7 — required-framework declaration

Declare the pivot complete only when:

- all post-check production IR uses Pliron;
- the VM and every backend consume verified `mojito.core`;
- `.mir` v2 is stable, deterministic, source-located, and backward compatible;
- ownership/drop semantics have exact differential parity;
- native parity reaches the roadmap threshold;
- two Pliron upgrade rehearsals have succeeded without a broad fork;
- default builds remain LLVM-free and have acceptable compile time/memory;
- fuzzing covers parser, verifier, conversions, and pass pipelines;
- crash reports can print a minimized canonical IR and pass pipeline;
- Mojito has a documented response if Pliron becomes unmaintained.

## Build and distribution

- Make core `pliron` required only at A4. It is Rust-only and ships in every
  build.
- Keep `pliron-llvm` and `llvm-sys` under `backend-pliron`; the default VM CLI
  must neither discover nor link LLVM.
- Linux remains the promotion platform first. Package an exact LLVM 22 toolchain
  or document distro packages and `LLVM_SYS_221_PREFIX`.
- macOS requires universal decision records for Homebrew/package LLVM, rpaths,
  deployment targets, codesigning, and arm64/x86_64 CI before support claims.
- Windows requires an LLVM distribution, MSVC ABI/linker testing, DLL/static
  linkage policy, path discovery, and CI before support claims.
- If distributing the compiler, prefer a hermetic bundled LLVM subset where
  licensing and size permit; otherwise make native support an installable
  component. A VM-only Mojito distribution remains small and Rust-native.

## Risk register

| Risk | Trigger | Mitigation / rollback |
|---|---|---|
| Upstream API churn | repeated expensive 0.x upgrades | adapter boundary, exact pins, scheduled rehearsals; freeze or revert to option B. |
| Maintainer concentration | prolonged inactivity or incompatible direction | fund/contribute upstream, maintain a small audited patch queue; reject a broad fork. |
| Missing legality/type conversion | conversions become hand-audited walks | implement locally behind one API and upstream before A4. |
| Weak tooling | IR bugs cannot be minimized/reproduced | build printer, pipeline parser, verifier-after-pass, reducer/fuzzer before cutover. |
| Compile-time/memory regression | Pliron object graph exceeds budgets | benchmark each stage; compact attributes/types, intern aggressively, or retain Rust MIR. |
| Semantic drift during port | ownership/drop/try behavior differs | dual pipelines, VM differential, lifecycle traces, randomized tests; never delete old path early. |
| LLVM leakage | default build links/discovers LLVM | dependency-tree guard separating `pliron` from `pliron-llvm`. |
| LLVM dialect gaps | broad local operation patch set | upstream narrow additions; use `mojito.abi` temporarily; fall back to Cranelift from core. |
| Artifact instability | canonical text changes across Pliron upgrades | own `.mir` syntax/version adapter or pin printer semantics independent of upstream IDs. |
| False Mojo analogy | architecture expands toward unsupported GPU goals | scope goals explicitly; add hardware dialects only behind demonstrated programs and funding. |

## Rejected alternatives

- **Immediate replacement of MIR:** too much simultaneous change, destroys the
  oracle boundary, and makes failures impossible to localize.
- **Parser direct to Pliron:** copies a reported Mojo choice without Mojito's
  needs; it would erase valuable AST and checked-semantic seams.
- **LLVM dialect as the stable waist:** loses ownership, nominal, exact literal,
  structured exception, and target-independent facts too early.
- **A one-to-one mechanical MIR dialect forever:** adds framework overhead
  without exploiting SSA, interfaces, passes, or progressive lowering. It is
  acceptable only as the A1 migration bridge.
- **Expose Pliron IR in the source language now:** locks user programs to an
  immature dialect ecosystem and creates compatibility obligations before the
  compiler substrate is proven.
- **Assume Pliron enables non-LLVM native output:** false today. Future
  Cranelift/SPIR-V paths are separate backends consuming core dialects.

## Smallest falsifiable proof

Before approving A4, implement A1 for a deliberately difficult vertical slice:

1. one generic struct containing an exact literal and reference field;
2. a generic method specialized twice;
3. a raising call inside `try/finally`;
4. a move followed by deterministic drop on each normal/error edge;
5. VM interpretation and LLVM execution from the same `mojito.core` module;
6. canonical print/parse/print and v1-MIR translation;
7. one optimization that removes dead scalar work without changing lifecycle.

This slice stresses every reason to retain a language-aware IR. The pivot is
falsified if Pliron cannot represent and verify it cleanly, if source locations
or lifecycle order become lossy, if conversion totality remains informal, if
the default build acquires LLVM, or if measured overhead is disproportionate.

## Final assessment

Pliron is up to being **Mojito's** required IR framework, provided Mojito wants
an extensible Rust-native middle end and accepts ownership of its dialects,
passes, tooling, and some upstream framework work. It is not currently up to
being **MLIR in general**, and therefore cannot make Mojito architecturally or
operationally equivalent to Mojo's MLIR-based heterogeneous stack.

Option A is feasible because Mojito's scope is narrower and the existing MIR,
VM, native ABI, and differential corpus provide unusually strong migration
oracles. The recommended commitment point is A4, not today: first prove that a
Pliron `mojito.core` can replace the current MIR representation without losing
semantic clarity, artifact stability, diagnostics, or build isolation.

## Primary sources

- Pliron repository and status: <https://github.com/pliron-org/pliron>
- Pliron API documentation: <https://pliron-org.github.io/pliron/>
- CUDA Oxide pipeline/status: <https://github.com/NVlabs/cuda-oxide>
- CubeCL current architecture: <https://github.com/tracel-ai/cubecl>
- MLIR language reference: <https://mlir.llvm.org/docs/LangRef/>
- MLIR dialect conversion: <https://mlir.llvm.org/docs/DialectConversion/>
- MLIR data layout: <https://mlir.llvm.org/docs/DataLayout/>
- MLIR LLVM target: <https://mlir.llvm.org/docs/TargetLLVMIR/>
- Mojo inline MLIR reference: <https://mojolang.static.modular.com/docs/reference/inline-mlir/>
- Modular Mojo FAQ: <https://docs.modular.com/stable/mojo/faq/>
