# Mojito Roadmap

This is the project's single task tracker: the direction of travel and a
dependency-ordered list of **unfinished** work only. Completed work does not
accumulate here — capabilities are recorded in
[`docs/features.md`](docs/features.md) (the authoritative support matrix),
user-visible history in [`CHANGELOG.md`](CHANGELOG.md), and lasting design
invariants in [`docs/architecture.md`](docs/architecture.md).

The north star is self-hosting: useful standard-library code should expose the
next missing compiler capability. Prefer the smallest honest language change
that unlocks a real library pattern, with positive and negative tests.

## Where Mojito Stands

Mojito has one production path: source is linked and elaborated, checked into
typed HIR, lowered to verified MIR, ownership- and liveness-analyzed, drop
elaborated, and executed by the register VM. The supported CPU surface includes
exact numeric literals, type/value/origin generics and heterogeneous packs,
scope-stable and lexical nested-pack specialization, refined and conditional
traits, recursively lifted typed closure environments, linear whole-pack
forwarding, generic-anonymous callable contracts, symbolic callable defaults,
residual callable specialization, origin-bearing references and current-model
unsafe pointers (the `unsafe_*` vocabulary, empty-`[]` dereference,
layout-based linear `std.memory` allocation with a tracked
`Allocation.unsafe_ptr()`, multi-element interior-domain origins carrying
the borrowed `Span`/`StringSpan` views — with current Mojo's strict slice
bounds, grapheme-cluster and Span element iteration, and the `@implicit`
List-to-Span conversion whose temporary refines its origin to the source —
and the experimental conservative `Origin._subtree` form, including
`Pointer(to=…)` through `ref` bindings),
collection-owned interior-origin generations, explicit lifecycle semantics, and
a self-hosted proof-subset standard library. Method-dispatched nominal
subscripts retain ordinary checked method selection in one complete verified
call contract, including effects, caller places, capture access, generic values,
and reference results; call-less index and slice operations name their exact
compiler-owned intrinsic family instead of asking the VM to infer one from a
runtime value. The narrow nominally typed `Slice.indices()` result still crosses
private Tuple storage through that explicit intrinsic bridge. Public
`List`, `Set`, `Dict`, heterogeneous `Tuple`, and current Mojo's private
Int/Scalar range family are nominal library structs; only compile-time lists
and the private heterogeneous runtime-pack carrier retain compiler-owned
aggregate representations.

[`docs/features.md`](docs/features.md) is the authoritative support matrix;
[`conformance/parity.tsv`](conformance/parity.tsv) and
[`docs/mojo-nightly.md`](docs/mojo-nightly.md) pin claims against real Mojo.

## Direction

Work proceeds in dependency order through the numbered sections below:

1. **Finish MIR-schema-prerequisite CPU semantics.** Complete — anything that
   could still change MIR value, constant, or instruction schemas has landed,
   ending with the cross-call transfer residues (type-carried and
   higher-order effects, conformer unions, the capture channel, and
   interior-precise destination domains).
2. **Catch up to current Mojo.** Mojo is a moving target: re-pin the nightly,
   re-probe the parity claims, and close the recorded divergences before
   freezing artifacts against a stale picture of the language. This is a
   recurring task — it reopens at every re-pin.
3. **Close and validate the textual MIR milestone.** Complete — the cached,
   post-drop verified artifact is the exact backend handoff and the full
   direct/artifact conformance gate is closed.
4. **Build a native backend.** Evaluate Pliron first as an optional lowering
   framework below MIR, promote it stage by stage toward LLVM output, and pivot
   to Cranelift only if the feasibility or scalar-native gates expose a material
   blocker.
5. **Grow the CPU standard library** demand-first against the stable MIR and
   native-runtime contracts.
6. **Packaging, artifacts, and developer tooling**, including compiled package
   artifacts and a reproducibility gate.

## Ordered Work

Every entry is in implementation order. The first unchecked checkbox is the
default next task.

### 1. Complete MIR-Schema-Prerequisite CPU Semantics

Complete: the MIR value, constant, and instruction schemas are settled
(cross-call transfer residues closed the last seam; see the "Cross-call loan
transfer" row in `docs/features.md` and the transfer-effects section of
`docs/architecture.md` for the frozen contract and its deliberate
residues). Later library/API and source-syntax growth must lower to the
frozen operations unless it deliberately reopens the schema.

### 2. Catch Up To Current Mojo (Recurring)

Mojo keeps changing under Mojito, so parity work recurs: whenever the pinned
nightly moves, re-pin [`docs/mojo-nightly.md`](docs/mojo-nightly.md), re-probe
the [`conformance/parity.tsv`](conformance/parity.tsv) claims against the new
compiler, and burn down the recorded divergences. Completing one pass deletes
the checkbox as usual; the next re-pin recreates it with the fresh divergence
list. Doing a pass before the textual-format freeze keeps artifacts from
encoding a stale picture of the language.

Governing rule: Mojito matches or subsets Mojo — it accepts what the audited
head accepts, with extensions tolerable only as (a) temporary bridges tracking
upstream's own deprecation state or (b) implementations of features on Mojo's
own roadmap/proposals, citing the upstream evidence in the parity records and
re-probed at every re-pin (e.g. expected struct extensions would qualify). The extension alignment sweep for the `ae386d1b204` audit is done
(see the changelog: `unified {...}`, bare `move:`, the competing `__setitem__`
pair, `def(...)`-typed storage, captured-Origin specialization values, and
unqualified stateful downward funargs now reject; the `objs[0](args)`
element-call gap recorded then has since been closed by the bare-spelling
re-dispatch). The `ae386d1b204` pass is complete, including its close-out
(probes resolved, the Confirmed Alignment list re-verified, and the full
differential run recorded in `docs/mojo-nightly.md`); the recorded divergences
to burn down next pass live in `conformance/parity.tsv` notes and the
`mojito-only`/`mojo-only` rows of `conformance/cases.tsv`: span
parameterization (`Span[T, _]`, `Imm`/`Mut` view aliases), `len(String)`
acceptance, bare `def(...)` parameter annotations, OwnedPointer `p[]`, the
`._subtree`-cast bridge, `capturing[...]`-annotated closure locals (upstream
stores closures un-annotated — needs closure escape inference for
un-annotated local bindings), and prelude-visible `Set` (upstream requires an
import for the name while set displays stay name-independent — needs the
display lowering to reach the stdlib Set through a compiler-internal identity
instead of the prelude binding).

### 3. Close And Validate The MIR Artifact Milestone

Complete: the textual MIR milestone is closed. `CompiledProgram` retains the
ownership-verified, drop-elaborated, re-verified `MirProgram` as one cached
artifact — the exact input to every backend — consumed by both execution and
canonical emission; `emit-mir | exec -` is the backend-independent
producer/consumer contract; and every shared runnable conformance case pins
direct execution, canonical print → parse → print byte equality, and artifact
execution with identical output and displayed bindings, with corpus-shrink
guards on the conformance and round-trip fixture sets (see the artifact rows of
`docs/features.md` and the textual MIR/VM assembly boundary section of
`docs/architecture.md`).

### 4. Native Backend: Pliron First, Cranelift On Material Failure

The artifact close-out has passed, so native-backend work is unblocked and
Pliron Stage 0 below is the default next task. Verified MIR is the stable
waist, the VM remains the semantic oracle,
and native work does not wait for complete standard-library, packaging, or Mojo
surface parity. Unsupported native behavior rejects with a contextual compile
diagnostic; it never silently falls back to the VM.

The preferred architecture is:

```text
source -> Compiler -> ownership-verified, post-drop verified MIR
                            |                 |
                            |                 +-> VM / canonical .mir artifact
                            v
                    Pliron lowering
                            |
             Mojito ops only where justified
                            |
                    Pliron LLVM dialect
                            |
            LLVM IR / bitcode / object / executable
                            |
                    versioned runtime ABI
```

Do not replace Mojito MIR with Pliron IR. Pliron is an optional lowering,
verification, transformation, and optimization framework below the serialized
MIR handoff. A backend consumes `MirProgram`; it does not import AST, HIR,
checker, ownership, or call-binding policy. The default VM build must continue
to work without Pliron or LLVM installed.

Pliron core is implemented in Rust, but its native LLVM path uses `llvm-sys`
and requires a compatible LLVM installation. It reduces direct C++ API and
ownership friction; it does not eliminate LLVM as a native toolchain
dependency. Keep this distinction explicit in build and user documentation.

#### Backend interface and dialect policy

Separate VM execution from native compilation. The native interface accepts
`&MirProgram`, a target description, and an output kind, and returns textual IR,
bitcode, an object, or an executable plus structured diagnostics. Add
`run --backend pliron` only when the JIT or executable path matches the VM for
the advertised subset. Existing `emit-mir` and `exec` behavior stays unchanged.

The first scalar spike lowers directly to Pliron's LLVM dialect. Introduce a
narrow `mojito` Pliron dialect only for demonstrated needs such as runtime
calls, checked traps, explicit error propagation, target-independent aggregate
constants, or lifecycle normalization. Do not reproduce the entire MIR schema
as a second operation set. Every custom operation needs textual syntax, a
verifier, negative coverage, and a total conversion rule; LLVM emission rejects
any residual illegal operation.

#### Shared semantic and ABI rules

Native layout has one owner shared by Pliron, Cranelift, and any later backend.
Specialized MIR functions map to deterministically mangled native symbols;
origin and ownership facts erase after validation, while explicit drop and
cleanup instructions remain executable behavior. References lower to a native
pointer plus only the runtime metadata their checked type requires. Strings use
a specified descriptor and constant pool. Aggregates use target-owned layouts,
never Rust's unspecified `repr(Rust)` layout.

Initially lower errors and `try`/`finally` as tagged outcomes and explicit CFG
edges so every success, error, return, and cleanup path is differential-testable.
Do not use platform unwinding until it has a separate semantic, ABI, and
portability specification.

The runtime is a small, independently versioned Rust library with a C ABI. It
must not expose the VM's internal `Value` representation. Its contract defines
size, alignment, ownership, nullability, allocation responsibility, failure
behavior, and ABI-version mismatch handling for every exported symbol.

#### Cross-stage testing and rollback

Every stage uses four complementary layers:

1. IR unit tests for each lowering, verifier, rewrite, conversion, ABI type,
   and unsupported diagnostic.
2. Canonical Pliron/LLVM snapshots with UTF-8, LF, and one trailing newline.
3. VM/native differential tests comparing stdout bytes, bindings or result,
   error category, and ordered lifecycle events at `O0` and optimized levels.
4. Artifact tests proving `.mir -> VM` and `.mir -> native` consume the same
   serialized program.

Track compile time, peak memory, IR size at each boundary, object size,
execution time, and supported/excluded MIR counts. Every stage is removable by
disabling its optional feature and backend modules without changing MIR, VM, or
source semantics.

- [ ] **Pliron Stage 0: feasibility, exact pin, and dependency isolation** —
  validate Pliron outside the production compiler before committing to it:
  - select exact, mutually compatible `pliron` and `pliron-llvm` releases, or
    one immutable Git revision if no release passes; retain the lockfile and
    record source checksums, upstream commit, Rust version, LLVM version,
    enabled features, required packages, and discovery configuration
  - audit the current tutorial/Kaleidoscope path and the exact present-day use
    of Pliron in CUDA Oxide and CubeCL rather than treating project names as
    maturity evidence
  - prove construction, parsing/printing, verification failure, rewriting,
    dialect conversion, LLVM module/bitcode export, object emission, and host
    execution with source-associated diagnostics
  - classify every required facility—operations, types, attributes, blocks,
    SSA, dominance, pass invalidation, conversion legality, LLVM dialect
    coverage, data layout, JIT/targets, and diagnostics—as supported, locally
    bridgeable, upstream gap, or blocker with evidence
  - prototype optional Cargo features and CI isolation without connecting the
    spike to `Compiler`; a clean default build must succeed with no LLVM
  - establish Linux as the first native host, with macOS and Windows advertised
    only after their independent object, linker, runtime, and execution gates

  Acceptance: pinned Linux CI emits and executes `main -> i32`; invalid IR
  reports a source-associated diagnostic rather than panicking; the default VM
  lane needs no native toolchain; and no required API needs a broad Mojito fork.

  Material failure means LLVM cannot be isolated from the default build,
  required operations/export cannot be implemented without a broad fork, the
  version/update burden is untenable, or source-aware verification cannot be
  preserved. A material failure skips the remaining Pliron stages and promotes
  the Cranelift fallback task below.

- [ ] **Pliron Stage 1: scalar MIR-to-native vertical slice** — connect the
  smallest production subset to the cached executable MIR:
  - support integer and Bool constants and arithmetic, comparisons, blocks,
    unconditional and conditional branches, direct calls, recursion, and return
  - lower directly to the LLVM dialect, verify before and after conversion, and
    emit canonical Pliron text, LLVM IR/bitcode, object, and host executable
  - add deterministic symbol mangling, MIR source-location propagation, a
    total supported-subset match, and contextual unsupported diagnostics
  - expose experimental `compile --backend pliron --emit ...` behind optional
    features; text may use stdout, while binary output requires an output path

  Acceptance: straight-line, diamond, loop, multi-function, and recursive
  examples match the VM; Pliron text round-trips byte-stably; repeated builds
  produce deterministic LLVM IR; invalid IR fails verification; and the backend
  imports no pre-MIR semantic representation.

  Passing this task authorizes broader Pliron work; it does not promote Pliron
  to the preferred user-facing backend. If this vertical slice exposes a
  material framework, LLVM-dialect, diagnostic, or distribution blocker, stop
  Pliron and promote the Cranelift fallback rather than maintaining two partial
  production backends.

- [ ] **Pliron Stage 2: complete scalar execution and conversion legality** —
  support all checked scalar operators and conversions, local storage,
  parameters/results, recursion, and supported scalar control flow; introduce
  Mojito dialect operations only where a demonstrated semantic boundary needs
  them; require full conversion with no residual illegal operations; define a
  conservative optimization pipeline and optional test-only host JIT.

  Acceptance: every eligible scalar `run` conformance case matches VM output,
  result/trap category, and bindings at `O0` and the initial optimized level; a
  generated capability manifest records every exclusion; and a guard test fails
  if eligible coverage unexpectedly shrinks.

- [ ] **Shared native target, layout, and runtime ABI** — before strings,
  aggregates, collections, or references become native, define:
  - target triple, CPU features, optimization level, output kind, and output
    path as checked build configuration
  - integer, Bool, floating-point, overflow, conversion, NaN, and signed-zero
    behavior
  - size, alignment, padding, aggregate field order, calling convention,
    parameter/result lowering, and deterministic symbol mangling
  - string, reference, pointer, allocation, output, and error representations
  - a versioned runtime C ABI with mechanically checked Rust/LLVM signatures
    and target-data layout tests

  Acceptance: Rust/runtime and generated native layouts and signatures agree;
  target-only cross checks work without executing foreign code; runtime symbols
  carry an inspectable ABI version; and no native ABI depends on VM `Value`.

- [ ] **Pliron Stage 3: runtime, strings, aggregates, allocation, and errors** —
  add constant pools, target-layout aggregates, printing, allocation and
  deallocation, checked traps, and explicit runtime error values through the
  shared ABI.

  Acceptance: eligible string, aggregate, allocation, and error fixtures match
  VM output and failure category; sanitizer runs find no leak, double free,
  misalignment, or use-after-free; and produced objects expose only the
  specified runtime symbols.

- [ ] **Pliron Stage 4: references, destruction, and exceptional control flow**
  — consume drop-elaborated MIR exactly as emitted; lower tagged success/error
  outcomes and every `try`/`finally` cleanup edge explicitly; preserve reference
  behavior without re-running ownership analysis; instrument ordered lifecycle
  events in the test runtime.

  Acceptance: eligible ownership/destruction cases have the same ordered
  create/drop/error trace as the VM; negative ownership cases fail before the
  backend; reference traps occur at the same semantic boundary; and nested
  errors, cleanup failures, and early returns have focused differential tests.

- [ ] **Pliron Stage 5: supported-language native parity** — grow one vertical
  slice at a time across specialized generics, retained callable forms,
  indirect calls and closures, collections, rich literals, references, and
  other missing MIR forms. Maintain a generated operation/type/runtime
  capability matrix and upstream narrowly scoped missing Pliron support rather
  than accumulating untracked local patches.

  Acceptance: canonical `.mir` artifacts behave identically through VM and
  native paths; the native backend never accepts rejected source; and promotion
  requires zero exclusions across Mojito's advertised runnable subset.

- [ ] **Pliron Stage 6: optimization and distributable artifacts** — define
  verified `O0` and release pass pipelines, analysis invalidation, object and
  executable linking, source-level debug locations, runtime packaging,
  reproducibility, and pre-declared compile-time, memory, code-size, startup,
  and runtime benchmark thresholds.

  Acceptance: optimized and unoptimized outputs pass the complete differential
  suite; native execution provides a material measured benefit without an
  unacceptable compile-time or size regression; clean builds are reproducible
  or have narrowly documented nondeterministic sections; and executables run
  without the compiler installed while diagnosing ABI mismatch clearly.

- [ ] **Pliron promotion decision** — promote Pliron from experimental to the
  preferred native backend only with semantic parity for all runnable corpus
  and conformance cases, no untracked MIR gaps, reproducible Linux tooling,
  tested/documented macOS and Windows status, acceptable benchmarks, no broad
  fork, at least one successful dependency-upgrade rehearsal, and a sustained
  CI period without unresolved correctness regressions. Promotion does not
  remove the VM or make Pliron a required internal compiler layer.

- [ ] **Cranelift fallback, only on material Pliron failure** — if Stage 0 or
  Stage 1 meets its explicit stop condition, record the evidence and implement
  the same scalar acceptance slice with Cranelift over verified MIR. Reuse the
  shared target/layout/runtime ABI and the same differential corpus. Prefer
  Cranelift when the governing goal becomes the shortest dependable route to
  machine code; do not build it in parallel merely as a second production
  backend. If Cranelift also fails materially, reassess direct LLVM, Melior,
  Inkwell, or a C/C++ source backend with a fresh decision record.

- [ ] **Native SIMD lowering** — after the selected backend reaches supported
  language parity, map completed SIMD semantics to native vectors where the
  target supports them and preserve defined scalar fallback behavior.

#### Pliron risks that remain promotion blockers

- API churn or upgrade cost disproportionate to Mojito's compatibility layer
- missing LLVM dialect/export coverage requiring a broad local fork
- LLVM discovery or linking that cannot be made reproducible per advertised OS
- semantic drift in output, errors, references, or drop ordering
- a Mojito dialect that mechanically duplicates MIR
- aggregate/data-layout disagreement between runtime and generated code
- optimization-only miscompilation
- undocumented runtime ABI lock-in
- stale real-project adoption evidence or an unsustainable upstream bus factor

The default response to a correctness failure is to keep Pliron experimental or
disable the offending optimization. The default response to an upstream or
distribution failure at Stage 0/1 is the Cranelift fallback, not erosion of the
MIR/VM contracts.

### 5. Grow The CPU Standard Library

- [ ] **Collection API parity** — grow List, Dict, HashDict, Set, HashSet, tuple,
  slice, optional/variant, and String result APIs
  (`replace`/`join`/`strip`/...) demand-first from conformance cases. For
  `Variant`, finish:
  - representation writing
  - fully generic TypeList-driven conditional protocol synthesis rather than
    adding compiler special cases for every standard-library method
- [ ] **Layout and LayoutTensor growth** — the CPU core is landed (bundled
  `layout` package; DType and frozen-struct value parameters; see
  `docs/features.md`). Grow demand-first:
  - origin-parameterized borrowed tensor views (the multi-element
    origin-bearing pointer substrate landed with the nightly-§5 views work;
    grow the tensor-view surface on it)
  - tile/slice/transpose views
  - SIMD `load/store[width]` on the landed SIMD machinery
  - the layout algebra
    (`composition`/`coalesce`/`blocked_product`/`logical_divide`)
  - `idx2crd`
  - rank gating via layout `where` predicates (comptime method evaluation on
    frozen struct values)
  - a public recursive `IntTuple`
  - mixing type parameters with DType/struct value parameters on one struct
- [ ] **HashSet growth and rehashing** — add load-factor growth while preserving
  deterministic behavior and value semantics.
- [ ] **Filesystem and I/O slice** — port representative file/path/stream APIs on
  the Writer and explicit-destroy foundations.
- [ ] **Time, random, and testing slices** — add deterministic testable cores and
  isolate host-dependent behavior behind runtime services.

### 6. Packaging, Artifacts, And Developer Tooling

- [ ] **Feature and target options** — expose checked CLI/build configuration and
  record it in artifacts and diagnostics.
- [ ] **Compiled package artifacts** — define and load a versioned `.mojoc`
  representation without making modules first-class runtime values. Complete the
  per-directory resolution order around the already implemented source choices:
  1. source package
  2. `.mojoc`
  3. source module
  4. legacy `.mojopkg`
- [ ] **Debugging metadata and inspection** — provide stack/source diagnostics,
  MIR inspection, and debugger-oriented value rendering.
- [ ] **Testing tools** — provide Mojito-native assertions, expected-error tests,
  and integration with the differential harness.
- [ ] **Distribution reproducibility gate** — make the release check rebuild,
  test, document, and reproduce conformance results using only the crates.io
  archive contents.

### Explicit Non-Goals For First-Pass Parity

- GPU programming and accelerator memory/execution models
- concurrency, parallelism, atomics, tasks, and distributed execution
- Python interoperability
- any backend IR as a *required* internal compiler layer (Pliron, Cranelift,
  and any other native backend are pursued below the verified-MIR waist, not as
  a mandatory IR the whole compiler is built on)
- legacy `fn`, `owned`, and other removed source spellings except for clear
  rejection diagnostics
- escaping closures and the removed `escaping` function effect; first-pass
  closure parity targets Mojo's current non-escaping capture-list model

## Task Lifecycle Policy

`roadmap.md` is the only task list. Do not create a parallel todo file, and do
not retain completed tasks here — checked boxes never accumulate.

- Unfinished work belongs in **Ordered Work** as an unchecked, outcome-oriented
  task. Add detailed design notes elsewhere only when they are needed to make a
  decision or preserve an architectural argument.
- A task is complete only when its implementation, focused positive and negative
  coverage, relevant documentation, and `scripts/check` all agree.
- In the same change that completes a task, **delete it from Ordered Work** and
  record the outcome in its documentation home: the capability row in
  `docs/features.md`, the user-visible entry in `CHANGELOG.md`, and any lasting
  design invariant in `docs/architecture.md` (or the relevant focused
  document). Update **Where Mojito Stands** only when the high-level picture
  changes.
- Delete obsolete implementation plans instead of retaining them as completed
  todos.
- Split or rewrite partially completed tasks so **Ordered Work** states only the
  remaining outcome. Never mark a broad task complete while leaving hidden
  follow-up work inside its description.
- Prefer one checkbox per independently demonstrable semantic outcome. Split a
  task when its parts require different compiler phases, can land without one
  another, have different backend dependencies, or need distinct conformance
  cases. A task may still span phases when those phases are inseparable from one
  end-to-end language guarantee.

## Working Rule

For each promoted task:

1. Start with a self-hosted library or small user-facing acceptance case.
2. Record the current failure with a focused test.
3. Implement the smallest compiler change that makes the program honest.
4. Add positive and negative coverage at the owning compiler phase.
5. Run `scripts/check` before marking the task complete.

Deferred work stays unchecked. Completion follows the lifecycle policy above.
