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
unsafe pointers (the `unsafe_*` vocabulary, empty-`[]` dereference, and
layout-based linear `std.memory` allocation),
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

1. **Finish MIR-schema-prerequisite CPU semantics.** Complete — anything that
   could still change MIR value, constant, or instruction schemas has landed,
   ending with the cross-call transfer residues (type-carried and
   higher-order effects, conformer unions, the capture channel, and
   interior-precise destination domains).
2. **Catch up to current Mojo.** Mojo is a moving target: re-pin the nightly,
   re-probe the parity claims, and close the recorded divergences before
   freezing artifacts against a stale picture of the language. This is a
   recurring task — it reopens at every re-pin.
3. **Freeze a textual MIR/VM assembly** once the checked-declaration + verified
   MIR contract is confirmed sufficient, giving backend-independent artifacts,
   snapshots, and a disassembler/assembler pair.
4. **Grow the CPU standard library** demand-first against that stable contract.
5. **Packaging, artifacts, and developer tooling**, including compiled package
   artifacts and a reproducibility gate.
6. **Native backends** — LLVM first, then the MLIR-family targets (MLIR and the
   Rust-native, MLIR-inspired Pliron), with Cranelift and eBPF following —
   validated differentially against the VM corpus.

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
re-probed at every re-pin (e.g. the `_finish` named-destructor convention
models the linear-types proposal; expected struct extensions would also
qualify). The extension alignment sweep for the `ae386d1b204` audit is done
(see the changelog: `unified {...}`, bare `move:`, the competing `__setitem__`
pair, `def(...)`-typed storage, captured-Origin specialization values, and
unqualified stateful downward funargs now reject; `objs[0](args)` is recorded
as a subset gap). The remaining pass works the prioritized changeset in
[`docs/mojo-nightly.md`](docs/mojo-nightly.md) (its §0–§8 hold the detailed
specifications and upstream evidence), in this order:

- [ ] **`UnsafeMaybeUninit` inline-uninit storage** — grow `UnsafeMaybeUninit`
  around the current `unsafe_write`/overloaded `unsafe_assume_init`/
  `unsafe_deinit`/`unsafe_forget` vocabulary (upstream
  [`b324feea`](https://github.com/modular/modular/commit/b324feeaa16bc13a12c0200164d1878fcfa64a87)).
  Split from the completed pointer/allocation migration because upstream's
  type is *inline* possibly-uninitialized storage: the VM's uninitialized
  tombstones exist only in the heap arena today, so a faithful port needs a
  new inline-uninit field capability across the checker, drop elaboration,
  and the VM rather than another spelling over heap slots.
- [ ] **Views and strict bounds (nightly §5)** — `Span` and canonical
  `StringSpan` (with `Imm`/`Mut` aliases); contiguous List/Span/String slices
  reject negative, out-of-range, or reversed bounds instead of normalizing;
  byte endpoints on UTF-8 boundaries; grapheme-cluster `StringSpan` yields
  from ordinary String iteration; strided List slicing keeps
  `StridedSlice.indices()` normalization and copied results. Emit
  `StringSpan`; accept `StringSlice` as an upstream compatibility alias.
- [ ] **Linear containers and owning APIs (nightly §6)** — loosen
  `Optional`/`Variant` to `AnyType` with `init_with=` placement construction
  and `deinit_with`; `clear_with`, displacement-returning `insert`, and
  consuming iteration by declared family; renames
  (`Variant.take` → `unwrap`, `OwnedPointer.take` → `into_inner`);
  quarantine or remove owned iteration for non-`Deinitable` elements where
  the head requires `Movable & Deinitable`. (Depends on lifecycle
  canonicalization; independent of Array and Pointer.)
- [ ] **Subtree origins and temporary-origin inference (nightly §7)** — add
  the experimental `Origin._subtree` as a separate conservative origin form
  beside the existing named interior generations; allow an origin-bearing
  `@implicit` conversion result to refine its origin from a register
  temporary; carry both facts explicitly through checked HIR and verified
  MIR. Follows the container work — the audited stdlib does not yet depend
  on it.
- [ ] **Scalar, SIMD, range, and generic vocabulary (nightly §8)** —
  generalize the Int-only `Range` proof subset to the Int/Scalar family;
  adopt `TypeList` `length`/`any`/`all` for variadic predicates; probe
  Tuple's public `*Ts` parameter name for compatibility. The SIMD half of
  the section is complete (`SIMDLength` landed; invalid widths already
  reject at checked elaboration).
- [ ] **Pass close-out** — in order:
  1. Run the open-question probes and the re-probe list in
     [`conformance/probes/`](conformance/probes/) against the audited build;
     resolve each per its header.
  2. Re-verify the "Confirmed Alignment" list in `docs/mojo-nightly.md`
     (add the permanent two-root namespace-directory module case and the
     caught-error `raise e` differential case).
  3. Full differential conformance run in a Pixi environment with the exact
     audited build; record `mojo --version` and both hashes with the
     results; update the `conformance/parity.tsv` header pins.
  4. Delete this section's checkboxes — the next re-pin recreates the pass
     with a fresh divergence list.

### 3. Stabilize Textual MIR/VM Assembly

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

### 4. Grow The CPU Standard Library

- [ ] **Collection API parity** — grow List, Dict, HashDict, Set, HashSet, tuple,
  slice, optional/variant, and String result APIs
  (`replace`/`join`/`strip`/...) demand-first from conformance cases. For
  `Variant`, finish `destroy_with`, representation writing, and fully generic
  TypeList-driven conditional protocol synthesis rather than adding compiler
  special cases for every standard-library method.
- [ ] **Layout and LayoutTensor growth** — the CPU core is landed (bundled
  `layout` package; DType and frozen-struct value parameters; see
  `docs/features.md`). Grow demand-first: origin-parameterized borrowed
  tensor views (needs multi-element origin-bearing pointers — today an
  origin-bearing `UnsafePointer` designates a single place),
  tile/slice/transpose views, SIMD `load/store[width]` on the landed SIMD
  machinery, the layout algebra (`composition`/`coalesce`/
  `blocked_product`/`logical_divide`), `idx2crd`, rank gating via layout
  `where` predicates (comptime method evaluation on frozen struct values),
  a public recursive `IntTuple`, and mixing type parameters with
  DType/struct value parameters on one struct.
- [ ] **Element-call dispatch for `value[i](args)`** — current Mojo dispatches
  the bare spelling as subscript-then-call on an indexable runtime value;
  Mojito parses it as compile-time parameter application and rejects with a
  parenthesization hint (a recorded subset gap pinned by
  `assets/type_error/callable_element_call_parses_as_parameter_application.mojo`).
  Closing it needs checker re-dispatch of the non-callable-base shape plus a
  subscript-then-indirect-call MIR lowering channel for `ExprKind::Call`.
- [ ] **HashSet growth and rehashing** — add load-factor growth while preserving
  deterministic behavior and value semantics.
- [ ] **Filesystem and I/O slice** — port representative file/path/stream APIs on
  the Writer and explicit-destroy foundations.
- [ ] **Time, random, and testing slices** — add deterministic testable cores and
  isolate host-dependent behavior behind runtime services.

### 5. Packaging, Artifacts, And Developer Tooling

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

### 6. Native Backends And Native-Only Semantics

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
