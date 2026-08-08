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
residual callable specialization, origin-bearing references and unsafe pointers,
collection-owned interior-origin generations, explicit lifecycle semantics, and
a self-hosted proof-subset standard library. Method-dispatched nominal
subscripts retain ordinary checked method selection in one complete verified
call contract, including effects, caller places, capture access, generic values,
and reference results; call-less index and slice operations name their exact
compiler-owned intrinsic family instead of asking the VM to infer one from a
runtime value. The narrow nominally typed `Slice.indices()` result still crosses
private Tuple storage through that explicit intrinsic bridge. Public
`List`, `Set`, `Dict`, `Range`, and heterogeneous `Tuple` values are nominal
library structs; only compile-time lists and the private heterogeneous
runtime-pack carrier retain compiler-owned aggregate representations.

[`docs/features.md`](docs/features.md) is the authoritative support matrix;
[`conformance/parity.tsv`](conformance/parity.tsv) and
[`docs/mojo-nightly.md`](docs/mojo-nightly.md) pin claims against real Mojo.

## Direction

Work proceeds in dependency order through the numbered sections below:

1. **Finish MIR-schema-prerequisite CPU semantics.** Anything that can still
   change MIR value, constant, or instruction schemas lands first.
   Current in-place operator dispatch, parameterized associated types and
   borrowed iterator origins, Unicode strings, SIMD, and CPU layout/tensor
   contracts are the remaining seams.
2. **Freeze a textual MIR/VM assembly** once the checked-declaration + verified
   MIR contract is confirmed sufficient, giving backend-independent artifacts,
   snapshots, and a disassembler/assembler pair.
3. **Grow the CPU standard library** demand-first against that stable contract.
4. **Packaging, artifacts, and developer tooling**, including compiled package
   artifacts and a reproducibility gate.
5. **Native backends** — LLVM first, then the MLIR-family targets (MLIR and the
   Rust-native, MLIR-inspired Pliron), with Cranelift and eBPF following —
   validated differentially against the VM corpus.

## Ordered Work

Every entry is in implementation order. The first unchecked checkbox is the
default next task.

### 1. Complete MIR-Schema-Prerequisite CPU Semantics

These tasks may change MIR value, constant, or instruction schemas. Complete
them before freezing the textual format; later library/API and source-syntax
growth must lower to the frozen operations unless it deliberately reopens the
schema.

- [ ] **String follow-ups** — the self-hosted core landed (nominal UTF-8
  `String`, keyword subscripts, `byte=`/`codepoint=` access,
  boundary-checked slicing, compare/hash/format; see `docs/features.md`).
  Remaining: `s[grapheme=i]` segmentation (decide the UAX #29 data
  strategy — generated tables module vs documented simplified rule) with a
  `Codepoint` wrapper type replacing the `Int` scalar result; lazy
  captured `TString` self-hosting; migrating the builtin literal
  operations onto the struct and splitting `StringLiteral` from `String`
  at the type level (annotation takeover, conversion retargeting,
  ordering on literals); and String result APIs
  (`find`/`split`/`startswith`/...) growing demand-first.
- [ ] **SIMD semantic completion** — finish dtype/literal conversions, masks,
  reductions, shuffles, and other CPU-visible VM semantics; migrate the brief
  `SIMDSize` spelling to current `SIMDLength` while retaining only an explicit
  compatibility policy for the deprecated alias.
- [ ] **CPU Layout and LayoutTensor semantics** — implement the target-independent
  type, indexing, and memory-view contracts required by CPU programs while
  leaving observable ABI layout and GPU memory spaces to later milestones.
- [ ] **Cross-call transfer residues** — the transfer-effect system is
  hardened (two-phase order-independent visibility; nested-def, unpack,
  and augmented-assignment guard coverage; see `docs/features.md` and
  `docs/architecture.md`); the remaining permissive gaps:
  indirect/function-value calls and abstract dispatch without a concrete
  body carry no effects (checked function types would need to carry
  them); destinations are root-granular (interior-precise dests would
  tighten sibling-field coexistence); the capture channel records no
  effects — a store through captured `self` inside a nested def is
  escape-checked but not transfer-recorded, invocation of a stored
  capturing-closure field/element remains unplumbed, and reading a
  capture-installed reference dies in the VM ("checked nominal subscript
  receiver is None", pre-existing) — one coherent capture-effects work
  item; and the chained-subscript verify fix still peels the loaded
  register's `Ty::Ref` at the check rather than retyping the register
  (retype only if a consumer needs it).

### 2. Stabilize Textual MIR/VM Assembly

- [ ] **Backend-ready MIR checkpoint** — confirm that checked declarations plus
  typed verified MIR are sufficient inputs, with no source-AST reconstruction,
  before freezing a serialized schema. Retain any final verification witnesses
  needed to validate abstract trait-dispatch signatures and checker-selected
  `ref`-to-`read` convention narrowing without trusting an unavailable source
  declaration or source binding-mutability fact. With bound-generic
  monomorphization complete, the abstract-dispatch surface is reachable only
  through the documented erased residue, and the concrete witness set to
  re-confirm is: `verify_iterator_result_adapter` (abstract `Next`/`TryNext`
  require the `CopyIteratorReference` adapter, concrete targets forbid it),
  the `GetIter` undeclared-prepare tolerance (only the
  `iterator_dispatch_symbol` spellings), the subscript abstract-target
  tolerance (receiver-membership skipped, full contract still verified), the
  `MethodCall` abstract-`__next__` adapter symmetry, `CallIndirect` abstract
  `__call__$ov$…` validation against the stored callable contract (the home
  of ref-to-read narrowing), and the direct-`Call` undeclared-callee
  tolerance (which also covers builtins and may deserve an allowlist here).
  Retain declared
  conventions for variadic overflow parameters if the serialized ABI exposes
  those conventions independently of their fixed-parameter prefix. Prove that
  every `MirPlace::through` derives from its exact source capability/loan, check
  `MirLoan::mutable` against that capability's permission, and cross-check each
  canonical interior origin with its executable place and declared reference
  origin before accepting assembled artifacts.
- [ ] **Text format schema** — specify versioning, deterministic identifiers,
  declarations, blocks, instructions, constants, types, and source locations.
- [ ] **Disassembler** — print every verified MIR program deterministically and
  add stable snapshots for representative programs.
- [ ] **Assembler parser and diagnostics** — parse the text format with precise
  source errors and no dependency on Mojo source syntax.
- [ ] **Artifact verifier integration** — run the canonical MIR semantic verifier
  on assembled programs and report artifact source locations before execution.
- [ ] **Lossless round trips** — require MIR → text → MIR equivalence across the
  full test corpus.
- [ ] **VM artifact execution** — run verified textual artifacts directly from
  the CLI.
- [ ] **Compiler/test integration** — expose dumps and use assembly snapshots and
  conformance artifacts as backend-independent contracts.

### 3. Grow The CPU Standard Library

- [ ] **Collection API parity** — grow List, Dict, HashDict, Set, HashSet, tuple,
  slice, and optional/variant APIs demand-first from conformance cases. For
  `Variant`, finish `destroy_with`, representation writing, and fully generic
  TypeList-driven conditional protocol synthesis rather than adding compiler
  special cases for every standard-library method.
- [ ] **HashSet growth and rehashing** — add load-factor growth while preserving
  deterministic behavior and value semantics.
- [ ] **Current memory and pointer API** — replace the legacy static
  `UnsafePointer[T].alloc[_aligned]` surface with free `alloc[T](...)`, add the
  current empty `pointer[]` dereference and initialization/deinitialization
  operations, and grow `Layout`/`Allocation`/`dealloc` as demanded by CPU
  library code. Lower the syntax to the existing typed Pointer MIR operations;
  do not add a second allocation representation.
- [ ] **Filesystem and I/O slice** — port representative file/path/stream APIs on
  the Writer and explicit-destroy foundations.
- [ ] **Time, random, and testing slices** — add deterministic testable cores and
  isolate host-dependent behavior behind runtime services.

### 4. Packaging, Artifacts, And Developer Tooling

- [ ] **Feature and target options** — expose checked CLI/build configuration and
  record it in artifacts and diagnostics.
- [ ] **Compiled package artifacts** — define and load a versioned `.mojoc`
  representation without making modules first-class runtime values. Complete the
  per-directory resolution order around the already implemented source choices:
  source package, `.mojoc`, source module, then legacy `.mojopkg`.
- [ ] **Debugging metadata and inspection** — provide stack/source diagnostics,
  MIR inspection, and debugger-oriented value rendering.
- [ ] **Testing tools** — provide Mojito-native assertions, expected-error tests,
  and integration with the differential harness.
- [ ] **Distribution reproducibility gate** — make the release check rebuild,
  test, document, and reproduce conformance results using only the crates.io
  archive contents.

### 5. Native Backends And Native-Only Semantics

The prioritized native targets are LLVM and the MLIR-family frameworks; Cranelift
and eBPF are later, lower-priority options. Every backend consumes the verified
MIR contract and is validated differentially against the VM/textual corpus.

- [ ] **LLVM backend** — the primary native target: lower the verified scalar CPU
  subset to LLVM IR and validate it differentially against the VM/textual corpus,
  then add stronger optimization/vectorization coverage.
- [ ] **Observable CPU layout and ABI rules** — define size, alignment, field
  layout, calling convention, and layout-marker semantics against native output;
  this is intentionally not a VM-parity prerequisite and is shared by every native
  backend.
- [ ] **MLIR backend** — lower verified MIR through MLIR dialects, reusing the
  layout/ABI rules above; an optional path for progressive lowering and reuse of
  the MLIR ecosystem's target coverage.
- [ ] **Pliron backend** — target [Pliron](https://github.com/pliron-org/pliron),
  a Rust-native, MLIR-inspired extensible IR framework whose LLVM dialect emits
  LLVM IR bitcode. As a pure-Rust path to native code it avoids a C++ MLIR/LLVM
  build dependency, making it an attractive in-tree lowering target once the MIR
  contract is stable.
- [ ] **Native SIMD lowering** — map completed SIMD semantics to native vectors
  where the chosen backend supports them, retaining scalar fallback behavior.
- [ ] **Later backends** — Cranelift (a fast, embeddable code generator) and eBPF
  are lower-priority options investigated after the LLVM/MLIR-family targets are
  stable; neither is a first-pass parity requirement.

### Explicit Non-Goals For First-Pass Parity

- GPU programming and accelerator memory/execution models
- concurrency, parallelism, atomics, tasks, and distributed execution
- Python interoperability
- MLIR as a *required* internal compiler layer (MLIR, Pliron, and LLVM are
  pursued as optional native backends below the verified-MIR waist, not as a
  mandatory IR the whole compiler is built on)
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
