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

The former single "Parameterized associated types and borrowed iterator origins"
task is split into the subtasks below. Its foundation and concrete substitution
have landed: trait/struct associated members retain type/value/origin parameter
lists (with the `//` infer-only boundary); a parameterized application such as
`Self.IteratorType[origin_of(self)]` is arity-validated; the checked type carries
the application's arguments (type, value, and *origin*, first-class in `TyArg`);
and a type-parameterized member instantiated by a conforming struct resolves
concretely through checked declarations, specialization, HIR, typed MIR,
verification, and the register VM (see `docs/features.md`).

- [x] **Concrete parameterized-associated-type substitution** — the checked
  `Ty::Assoc` carries application arguments (`TyArg` gained a first-class `Origin`
  variant), and a conforming struct's parameterized member resolves concretely by
  substituting them into the member's lowered template. A *type*-parameterized
  member (`C.Wrap[T]` → `List[T]`) resolves end-to-end and runs. Substituting an
  *origin* argument supplied as `origin_of(self)` now resolves too (see the
  self-origin note in the borrowed-iteration subtask below). One carried-over
  limitation remains: *value*-parameter forwarding into another parameterized
  struct (`Fixed[n]`) is blocked by a pre-existing generic value-forwarding gap
  unrelated to associated types.
- [ ] **Generic borrowed reference iteration and the borrowed `Iterable`
  protocol** — migrate the bundled borrowed `Iterable` to current Mojo's
  origin-parameterized `IteratorType[iterable_mut: Bool, //, iterable_origin:
  Origin[mut=iterable_mut]]` with `__iter__(ref self) ->
  Self.IteratorType[origin_of(self)]` (the trait plus the Range/List/Set/Dict
  conformances). *Self-origin resolution has landed:* a trait method's abstract
  `origin_of(self)` lowers to a symbolic self-origin, so a conforming struct's
  origin-parameterized associated member (`Self.IteratorType[origin_of(self)]`)
  resolves concretely and conformance succeeds — including when a conformer spells
  that application directly as its own `__iter__(ref self)` return type. A
  borrowed temporary now also keeps distinct retained-source-owner and iterator
  slots (instead of overwriting its only owner during normalization), so its
  `__del__` runs after the loop and a future origin-bearing iterator has a live
  source to loan. *Returning a struct-origin-parameter reference has landed:* a
  method may return a `ref[origin] T` field/binding whose origin is a struct
  origin parameter (the handle names its own borrowed region), which a
  reference-yielding `__next__` needs. *Projecting through a `ref[origin]`
  aggregate across a return now executes:* a reference indexed/projected out of a
  `ref[origin] <aggregate>` field (`self.src[i]`) is re-rooted by the VM at the
  borrowed storage, so it survives the accessor frame — including when the receiver
  is a `mut`/`ref self` handle to a caller frame. *Reading a `ref[origin]` field's
  referent under a `mut`/`ref self` receiver now works too* (subscript, `len`, …):
  the borrowed-receiver runtime alias no longer leaves the field load as its stored
  handle, so a `mut self` `__next__`'s `len(self.src)`/`self.src[i]` value reads
  succeed. *A reference returned from a struct method and bound to a `ref` local
  now keeps its source alive:* the returned reference's struct origin parameter
  resolves to the origin the receiver's `ref[o]` field borrows, so the loan roots
  at the ultimate owner and it is not dropped while the reference is live — so a
  `mut self` reference-yielding accessor (`def take(mut self) -> ref[o] Int: return
  self.src[i]`) bound to `ref` locals now reads and writes through end-to-end.
  *A `for` loop over a user-defined reference-yielding iterator now executes:* the
  loop invokes `__iter__`/`__next__` with the loop frame reachable (previously the
  synchronous `call_frame` path drove the callee with its caller popped out of the
  frame stack, so a user iterator holding a `ref` into the loop frame could not
  dereference it — `stale reference to frame N`), and a borrowed `__iter__(ref
  self)` receives a `ref self` handle so the iterator's borrow roots at the live
  loop frame. The yielded reference flows through the loop as a handle. This holds
  for an owned-temporary source (`for x in Numbers(3)`), which is retained and
  dropped exactly once after the loop. *A *named* source (`for x in nums`) is now
  borrowed, not copied:* it binds the source slot to a genuine reference (`MakeRef`)
  and records the whole-source dependency as a shared loan on the iterator, so the
  source is not copied, stays live through the loop without the `KeepAlive` hack,
  and mutating it during iteration is rejected as a loan conflict. What remains: an
  origin-bearing *pointer* deref return (`self.p[0]`) is still rejected (its place
  lowering keeps an offset-0 index the runtime cannot yet forward); migrate the
  bundled borrowed `Iterable` and the Range/List/Set/Dict conformances to the
  origin-parameterized shape; and remove the concrete List/Set/Dict
  collection-specific borrow bridges and the List-only `for ref` bridge. Cover
  mutable origins, generic bounds, structural invalidation, and escape rejection.
  The owned `IterableOwned` protocol already exposes monomorphic
  `IteratorOwnedType`; only the borrowed contract remains on the legacy monomorphic
  `Iter` member.
- [ ] **Owned iteration of linear elements** — in the owned path, permit a List
  of non-`ImplicitlyDeletable`/linear elements when every element is transferred
  by guaranteed exhaustion; reject only control-flow paths that can abandon a
  residual linear iterator, with its remaining obligations reported explicitly.
- [ ] **Self-hosted Unicode String** — define storage; current explicit
  `s[byte=i]`, `s[codepoint=i]`, and `s[grapheme=i]` indexing plus Unicode
  slicing; comparison, hashing, and formatting without VM-only semantics;
  distinguish compile-time `StringLiteral`, lazy captured `TString`, and
  explicit runtime `String` materialization. Bare positional `s[i]` remains
  rejected because a UTF-8 offset is ambiguous.
- [ ] **SIMD semantic completion** — finish dtype/literal conversions, masks,
  reductions, shuffles, and other CPU-visible VM semantics; migrate the brief
  `SIMDSize` spelling to current `SIMDLength` while retaining only an explicit
  compatibility policy for the deprecated alias.
- [ ] **CPU Layout and LayoutTensor semantics** — implement the target-independent
  type, indexing, and memory-view contracts required by CPU programs while
  leaving observable ABI layout and GPU memory spaces to later milestones.

### 2. Stabilize Textual MIR/VM Assembly

- [ ] **Backend-ready MIR checkpoint** — confirm that checked declarations plus
  typed verified MIR are sufficient inputs, with no source-AST reconstruction,
  before freezing a serialized schema. Retain any final verification witnesses
  needed to validate abstract trait-dispatch signatures and checker-selected
  `ref`-to-`read` convention narrowing without trusting an unavailable source
  declaration or source binding-mutability fact, and retain declared
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
