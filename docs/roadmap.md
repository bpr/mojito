# Mojito Roadmap

The project's single task tracker: an ordered checklist of **unfinished** work
only. Completed work does not accumulate here — the supported surface (and the
"Where Mojito Stands" overview) lives in [`docs/features.md`](features.md),
user-visible history in [`CHANGELOG.md`](../CHANGELOG.md), and lasting design
invariants in [`docs/architecture.md`](architecture.md) (whose "Native Backend
Contract" section holds the backend architecture, dialect policy, and
cross-stage testing contract). The north star is self-hosting: prefer the
smallest honest language change that unlocks a real library pattern, with
positive and negative tests.

Sections and the checkboxes inside them are in implementation order; the first
unchecked box is the default next task. Sections marked *(recurring)* or
*(any order)* are exceptions: they are not constrained by the main ordering.

## Ordered Work

### 1. Native Backend: Pliron Stage 5 — Supported-Language Native Parity

Verified MIR is the stable waist, the VM remains the semantic oracle, and
unsupported native behavior rejects with a contextual compile diagnostic —
never a silent VM fallback. Stages 0–4 (feasibility through references,
destruction, and exceptional control flow) are complete; designs and recorded
divergences live in `docs/notes/pliron-stage0.md` … `pliron-stage4.md`, the
shared target/layout/runtime contract in [`docs/native-abi.md`](native-abi.md),
and the generated gates in `conformance/pliron-parity.tsv` (the exe/raise
differential manifest with ratcheting exclusion guards) and
`conformance/pliron-capability.tsv` (the per-instruction/type/runtime-symbol
capability matrix). Stage 5 closes the remaining exclusions one vertical slice
at a time; every slice lands with fixtures, ratcheted manifest guards, flipped
capability rows, and a slice record in `docs/notes/pliron-stage5.md`.

Stage 5 acceptance: canonical `.mir` artifacts behave identically through VM
and native paths, the native backend never accepts rejected source, and
promotion requires zero exclusions across Mojito's advertised runnable subset.

- [ ] **Backend-side monomorphization** — a feature-independent
  `src/native/mono.rs` pass walks the instantiation graph from the entries,
  infers per-call-site type/value environments from concrete receiver and
  argument types, and clones functions and structs under instance mangles; the
  VM's runtime method-resolution policy is extracted into `src/symbol.rs` and
  shared, so native dispatch can never diverge from the oracle. The canonical
  `.mir` artifact and the VM are untouched.
- [ ] **Iterator protocol** — `GetIter`/`HasNext`/`Next`/`TryNext`, the
  reference-result copy adapter, and prepare-chain-typed iterator slots,
  including raising iterators over the tagged-outcome ABI.
- [ ] **Pointer and uninit storage intrinsics, builtins** — pointer-storage
  take/destroy, inline uninit storage with per-slot flags, `UnsafePointer`
  allocation, and the `len`/`abs`/`divmod`/`input`/`__floor__` builtins
  (`input` adds a runtime `mjrt_read_line`; ABI bump).
- [ ] **Collections** — general nominal and intrinsic subscripts
  (`Index`/`Slice`/`MultiIndex`/`MultiSet`), keyword/default call contracts,
  variadic direct calls and runtime packs, and per-field initialization flags
  for partial-move drops; unlocks the List/Dict/Set/Tuple/Array fixture
  clusters.
- [ ] **Retained callables, closures, indirect calls** — two-word
  `{invoke, env}` native `Func` values with per-function thunks matching the
  VM's captures-as-leading-arguments contract; nominal callable structs
  devirtualize to direct `__call__` calls during monomorphization.
- [ ] **Variant, scalar-semantics SIMD, move residues** — all Variant owning
  operations over the existing tag/payload layout, SIMD values as element-wise
  scalar aggregates (native vector types stay in the later SIMD task), user
  `__moveinit__` moves, and the remaining dynamic-residue drops.
- [ ] **Zero-exclusion burn-down** — fix the long tail until `excluded == 0`
  over `assets/ok` + `assets/ownership_ok`, extend the raise gate to every
  runnable `assets/runtime_error` fixture, assert the final guard state, and
  delete this Stage 5 section in the completing change.

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
  Pliron a required internal compiler layer. Standing promotion blockers:
  disproportionate upstream churn, missing LLVM dialect/export coverage
  forcing a broad local fork, irreproducible LLVM discovery or linking,
  semantic drift in output/errors/references/drop ordering, a Mojito dialect
  that mechanically duplicates MIR, runtime/codegen layout disagreement,
  optimization-only miscompilation, undocumented runtime ABI lock-in, or
  stale upstream adoption evidence.

- [ ] **Cranelift fallback** *(conditional — only on material Pliron
  failure)* — record the evidence, then implement the same acceptance slices
  with Cranelift over verified MIR, reusing the shared target/layout/runtime
  ABI and the same differential corpus. Prefer Cranelift when the governing
  goal becomes the shortest dependable route to machine code; do not build it
  in parallel merely as a second production backend. If Cranelift also fails
  materially, reassess direct LLVM, Melior, Inkwell, or a C/C++ source backend
  with a fresh decision record.

- [ ] **Native SIMD lowering** — after the selected backend reaches supported
  language parity, map completed SIMD semantics to native vectors where the
  target supports them and preserve defined scalar fallback behavior.

### 2. Catch Up To Current Mojo *(recurring — reopens at every nightly re-pin)*

Mojo is a moving target. Whenever the pinned nightly moves: re-pin
[`docs/mojo-nightly.md`](mojo-nightly.md), re-probe the
[`conformance/parity.tsv`](../conformance/parity.tsv) claims against the new
compiler, and burn down the recorded divergences (tracked in the
`parity.tsv` notes and the `mojito-only`/`mojo-only` rows of
`conformance/cases.tsv`). Governing rule: Mojito matches or subsets Mojo — it
accepts what the audited head accepts, with extensions tolerable only as
temporary bridges tracking upstream's own deprecation state or as cited
implementations of features on Mojo's own roadmap, re-probed at every re-pin.
The `ae386d1b204` pass is complete; the next re-pin recreates this section's
checkbox with the fresh divergence list.

### 3. Grow The CPU Standard Library *(demand-first — any order)*

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

### 4. Packaging, Artifacts, And Developer Tooling *(any order unless noted)*

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
- [ ] **Distribution reproducibility gate** *(last — depends on the above)* —
  make the release check rebuild, test, document, and reproduce conformance
  results using only the crates.io archive contents.

## Task Lifecycle Policy

`roadmap.md` is the only task list. Do not create a parallel todo file, and do
not retain completed tasks here — checked boxes never accumulate.

- Unfinished work belongs in **Ordered Work** as an unchecked, outcome-oriented
  task; detailed design notes live elsewhere (implementation plans, or
  `docs/notes/` when they preserve an architectural argument).
- A task is complete only when its implementation, focused positive and negative
  coverage, relevant documentation, and `scripts/check` all agree. In the same
  change, **delete it from Ordered Work** and record the outcome in its
  documentation home: the capability row in `docs/features.md`, the
  user-visible entry in `CHANGELOG.md`, and any lasting design invariant in
  `docs/architecture.md` (or the relevant focused document).
- Split or rewrite partially completed tasks so **Ordered Work** states only
  the remaining outcome; never mark a broad task complete while leaving hidden
  follow-up work inside its description. Prefer one checkbox per independently
  demonstrable semantic outcome, splitting when parts need different phases,
  can land separately, or need distinct conformance cases.

## Working Rule

For each promoted task:

1. Start with a self-hosted library or small user-facing acceptance case.
2. Record the current failure with a focused test.
3. Implement the smallest compiler change that makes the program honest.
4. Add positive and negative coverage at the owning compiler phase.
5. Run `scripts/check` before marking the task complete.

Deferred work stays unchecked. Completion follows the lifecycle policy above.
