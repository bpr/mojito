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

The artifact close-out, the Pliron Stage 0/1/2/3 gates, and the shared native
target/layout/runtime-ABI milestone have passed, so Pliron Stage 4 below is
the default next task. Verified MIR is the stable waist, the VM remains the semantic oracle,
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
bitcode, an object, or an executable plus structured diagnostics.
`run --backend pliron` executes only the advertised subset natively (since
Stage 3: scalars, strings, aggregates, allocation, printing, and unhandled
raises) and rejects everything else with a contextual diagnostic. Existing
`emit-mir` and `exec` behavior stays unchanged.

The first scalar spike lowers directly to Pliron's LLVM dialect. Introduce a
narrow `mojito` Pliron dialect only for demonstrated needs such as runtime
calls, checked traps, explicit error propagation, target-independent aggregate
constants, or lifecycle normalization. Do not reproduce the entire MIR schema
as a second operation set. Every custom operation needs textual syntax, a
verifier, negative coverage, and a total conversion rule; LLVM emission rejects
any residual illegal operation.

#### Shared semantic and ABI rules

Implemented and normative in [`docs/native-abi.md`](docs/native-abi.md): one
layout owner (`src/native/`) shared by Pliron, Cranelift, and any later
backend; checked build configuration; defined scalar/overflow/conversion
semantics; deterministic mangling; string/reference/pointer/allocation/
output/error representations; and the independently versioned `mojito-runtime`
C ABI (which never exposes the VM's internal `Value`), mechanically checked
from both the Rust and LLVM sides. Origin and ownership facts erase after
validation, while explicit drop and cleanup instructions remain executable
behavior. Errors and `try`/`finally` lower as tagged outcomes and explicit CFG
edges; platform unwinding stays out until it has its own semantic, ABI, and
portability specification.

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

Stage 0 (feasibility, exact pin, and dependency isolation) is complete — see
`docs/notes/pliron-stage0.md` for the pin record, ecosystem audit, facility
classification matrix, and go verdict. The spike lives in
`spikes/pliron-stage0/` behind `scripts/check-pliron-spike`; the default lane
remains LLVM-free (guarded by `tests/backend_isolation_test.rs`).

Stage 1 (scalar MIR-to-native vertical slice) is complete — the feature-gated
`src/backend/pliron/` backend compiles the scalar subset from the cached
post-drop artifact through the LLVM dialect to LLVM IR, bitcode, objects, and
executables via `mojito compile --backend pliron`, with VM parity pinned by
the JIT differential over `assets/ok/pliron_*` fixtures. See
`docs/notes/pliron-stage1.md` for the design, mangling scheme, and recorded
VM/native divergence policies; the LLVM lane's gate is `scripts/check-pliron`.
Passing Stage 1 authorizes broader Pliron work; it does not promote Pliron to
the preferred user-facing backend.

Stage 2 (complete scalar execution and conversion legality) is complete — the
backend lowers the full checked scalar operator and conversion surface over
Int, UInt, Float64, and Bool (keyword arguments and constant defaults bind
through the shared call-slot matcher), guards the checked div/mod-by-zero and
pow-exponent traps as explicit exit-code trap blocks (no Mojito dialect op
was needed), and adds `O0`/`O1` levels, a typed test JIT, and
`run --backend pliron` for the print-free subset. The generated capability
manifest `conformance/pliron-scalar.tsv` records every fixture's eligibility
or exclusion with shrink guards, pinned by the VM/native value and
trap-category differentials in `tests/pliron_backend_test.rs`. See
`docs/notes/pliron-stage2.md` for the semantics tables, divergence records,
and the for-range exclusion rationale.

The shared native target, layout, and runtime ABI milestone is complete — the
normative contract lives in [`docs/native-abi.md`](docs/native-abi.md), owned
in code by `src/native/` (checked build configuration incl. `--target`, the
layout engine, mangling, and the runtime contract table) and the versioned
`crates/mojito-runtime` C-ABI library that every produced executable links
and exposes via the inspectable `mjrt_abi_version` symbol. Integer overflow
is now defined two's-complement wrapping on both backends (the recorded
VM/native divergence closed), and Rust-side plus LLVM-side target-only cross
checks pin the layouts, signatures, data-layout string, and exported symbols
without executing generated code.

Stage 3 (runtime, strings, aggregates, allocation, and errors) is complete —
the backend lowers string-literal constant pools (private `mjstr_*` globals,
compile-time literal folds), `print` composed byte-exactly through the
runtime's `mjrt_fmt_*`/`mjrt_write_stdout` family, target-layout aggregates
(struct/tuple storage, fieldwise and `__init__` construction, resolved
methods with `mut self` write-back, copies through compiled `__copyinit__`,
drops through compiled `__deinit__`), heap allocation (`unsafe_alloc` and
pointer subscripts over ABI version 2's headered allocator and size-less
`mjrt_free`), the nominal `String` (its literal and copy constructors are
native bridges; `__deinit__` compiles from real MIR), and unhandled raises
through `mjrt_unhandled_error` (exit category 5, the explicit pre-Stage-4
error contract). Runtime traps route through `mjrt_trap`. Acceptance is
pinned by `tests/pliron_backend_test.rs`'s Stage 3 gate over
`conformance/pliron-stage3.tsv`: every eligible `assets/ok` fixture matches
VM stdout bytes at `O0`/`O1` and runs AddressSanitizer/LeakSanitizer-clean,
raise fixtures match the failure category and stderr, and produced
executables expose only the contract-table runtime symbols. Design and
divergence records: `docs/notes/pliron-stage3.md`.

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
  and conformance cases, no untracked MIR gaps, reproducible tooling on the
  supported Linux target, acceptable benchmarks, no broad fork, at least one
  successful dependency-upgrade rehearsal, and a sustained CI period without
  unresolved correctness regressions. Promotion does not remove the VM or make
  Pliron a required internal compiler layer.

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
- LLVM discovery or linking that cannot be made reproducible on the supported
  Linux target
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
