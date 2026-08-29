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

### 1. Native Backend: Pliron Stage 6 — Optimization and Distribution

- [ ] **Pliron Stage 6: dependency-upgrade rehearsal** — the one open
  acceptance item, blocked on the first upstream pliron/llvm-sys release
  newer than the current pins (0.17.0 / llvm-sys 221.0.1). The procedure
  and the rest of the completed acceptance evidence (full gates,
  parity/sanitizer lanes at both profiles, pinned-runner baseline with
  passed budgets, container-based compiler-free execution) are recorded
  in `docs/notes/pliron-stage6.md`.

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

  Close this checkbox with a written promotion decision record (a
  `docs/notes/` design note, like the stage notes): walk each criterion
  above and cite its evidence — the parity manifest's VM/native oracles at
  both profiles with sanitizers, Stage 5's zero native exclusions, the
  reproducibility and container evidence plus the committed baseline and
  passed budget table (`docs/notes/pliron-stage6.md`, "Acceptance
  evidence"), and the pinned no-patch upstream consumption — then sweep
  the standing blockers confirming none applies, and state the
  recommendation. Two inputs must accrue before the record can be
  written: the dependency-upgrade rehearsal above, and the sustained
  regression-free period, which accumulates from continued green gate
  runs while other roadmap work proceeds (logged in the stage 6 note's
  "Regression-free period accrual" section).

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

The `a79fbdf59f2` pass (2026-08-26, Mojo `1.1.0.dev2026082605`) is complete;
its close-out record lives in `docs/mojo-nightly.md`. The next re-pin
recreates this section's checkbox with the fresh divergence list.

### 3. Grow The CPU Standard Library *(demand-first — any order)*

- [ ] **Collection API parity** — grow tuple, slice, optional/variant, and
  String result APIs (`replace`/`join`/`strip`/...) demand-first from
  conformance cases (the 2026-08 pass landed the hashed insertion-ordered
  Dict, retired HashDict/HashSet, and grew the List/Dict/Set method
  surfaces — see `docs/features.md`). For `Variant`, finish:
  - representation writing
  - fully generic TypeList-driven conditional protocol synthesis rather than
    adding compiler special cases for every standard-library method

  Deferred from the 2026-08 Dict/Set/List pass with no shared blocker
  (each recorded in `conformance/parity.tsv` notes):
  - relaxing K/V/element bounds toward upstream's Movable-only KeyElement
    (a List-`AnyType`-style per-API `where` pass; sequence it after — or
    with — the transfer-convention item below, which rewrites the same
    signatures and call sites)
  - bare `if collection:` truthiness (relax the checker's `expect_bool`
    condition positions to dispatch `__bool__`; the `Bool(x)` lowering
    already exists)
  - `__reversed__`/`reversed()` (needs a short investigation: possibly
    self-hostable as a `Reversible` trait plus a prelude free function —
    no compiler protocol exists today)
  - `write_repr_to` (blocked on deciding a `repr` surface at all)
  - `(*, unsafe_uninit_length)` construction/resize (blocked on an
    uninit-element storage story for List, MaybeUninit-adjacent)

- [ ] **Parity-unblocking infrastructure** — compiler-side features that
  each unblock several recorded parity gaps at once. These are where
  "Mojito rejects/diverges from valid Mojo" clusters today; every item
  lists what it unlocks so slices can be chosen by leverage. The items
  below are in **recommended execution order**, not fan-out order. The
  load-bearing dependencies: the candidate-replay overload fix must land
  **before** the owned-`var` transfer convention (that convention injects
  mandatory `^`-moves at owned-parameter call sites in user code, which is
  exactly what trips the candidate-replay leak, and the private-helper
  workaround is not available to user code); the `None`→`Optional`
  coercion is a prerequisite for the String slice; the bound relaxation is
  paired with the transfer convention; and the hasher work is sequenced
  next to the transfer convention so the shared Dict/Set/List signatures
  are rewritten once. The three small self-contained items lead because
  they are cheap and stop rejecting valid programs immediately. The
  ref-field adapter follow-ups are a **parallel track** (origin/ref-system
  refinements plus bug fixes, not coupled to the API-parity sequence) and
  are listed last with their own internal order.
  - **`None` → `Optional` coercion in parameter defaults** — checker +
    VM (oracle) DONE: `Optional`'s `@implicit NoneType` constructor makes
    `None` coerce in binding/argument/default positions, the default-value
    check falls back to `@implicit` conversions, and an omitted-arg default
    materializes by running the converting constructor
    (`CheckedConst::Construct`). This unlocks the upstream signature pattern
    the String result-API family (`split`/`find` variants) and
    `index`/`try_index` `start`/`stop` keywords use, and is a prerequisite
    for the String slice. Remaining: native (pliron) lowering of a
    converting-constructor default — the default-fill sites reject aggregate
    defaults today (`checked_const_value` errors on `Construct`), needing a
    ctor-call emission at default-fill (reuse `lower_call`; the `NoneType`
    arg is `LowerTy::ZeroSized`). A native-backend concern, not an
    acceptance gap.
  - **A `size_of` builtin** (the shared native ABI layout tables in
    `src/native/` can answer it). Tiny and self-contained. Unlocks
    `List.byte_length` and upstream memory-oriented code that spells
    `size_of[T]()`.
  - **`|= &= ^=` augmented-assignment tokens** (grammar change:
    `docs/grammar.md` first, then lexer/parser/AST/checker/MIR).
    Self-contained. Unlocks the Dict/Set `__ior__`/`__iand__`/`__ixor__`
    family, and — broader — augmented bitwise assignment on Int/UInt,
    which is upstream-valid integer code Mojito rejects today.
  - **Overload-machinery hardening** — the remaining candidate-replay
    ownership-leak defect that gates wide API parity (the constructor
    disambiguation and same-arity method-key defects are closed). Do this
    before the owned-`var` transfer convention below. A `^`-moved argument
    at an overloaded call site corrupts the replay bookkeeping when the
    argument is feasible for both a consuming candidate and a candidate
    that borrows it through a ref-conversion (`List` matching consuming
    `extend(var other: Self)` while also converting to the borrowing
    `extend(elements: Span[Self.T])`): the losing borrow candidate's
    source-borrow survives into the winning consuming transfer, so
    ownership analysis reports "use of 'other' after transferred". First
    step is to pin the exact leaking map and write site — the mechanism is
    not yet confirmed. The free-function candidate loop
    (`call_inference.rs`) snapshots and restores five effect maps across
    candidates ("speculative candidates leak marks") while the method-call
    loops (`method_calls.rs`) snapshot nothing, so the snapshot gap is the
    leading suspect; but candidate scoring infers with `record=false` and
    the symptom re-surfaces from the origin/place-resolution path, so
    confirm the site before choosing among a scoring-time speculation
    guard, extending the method-loop snapshot, or a winner-replay/origin-
    solving fix. Worked around via the single-candidate
    `List._extend_moving` drain behind the overloaded `extend` surface.
  - **Owned-`var` transfer convention** — upstream requires `^` or
    `.copy()` to pass a non-ImplicitlyCopyable value to an owned
    parameter; Mojito implicitly copies, so `p + q` on Lists accepts
    here and rejects upstream. Closes a whole convention-level
    acceptance-divergence class rather than one API; large fixture blast
    radius, so schedule it as its own pass, land the candidate-replay
    overload fix first, and pair it with the bound relaxation above.
  - **Hasher-based `Hashable` and `std.hashlib` alignment** (trait
    signature `__hash__(self, mut hasher: Some[Hasher])` with a
    reflection default; AHasher; `std.hashlib` module identity). Sequenced
    next to the transfer convention because it rewrites the same
    Dict/Set/List signatures. Mojito's `__hash__() -> UInt` +
    `std.hashing` is fork surface: upstream user structs declaring the
    hasher-based method are rejected today — a direct acceptance gap for
    real upstream code. Unlocks: accepting those programs; upstream-parity
    `Dict.__hash__`/`List.__hash__`/`Set.__hash__` (do not implement these
    on the fork protocol first); hash-value parity; and, together with the
    `H` hasher type-parameter arity, the full `Dict[K, V, H]`/`Set[T, H]`
    signature.
  - **Parametric statics** — static-method dispatch on parameterized
    nominal types (`Dict[Int, Int].fromkeys(...)`); the checker's
    TypeApply statics stop at the pointer family, and a bare `Dict`
    identifier is not an expression binding. Fully independent, low
    fan-out — do it last or opportunistically. Unlocks `Dict.fromkeys`
    (already stdlib-expressible) and the parametric-statics gap recorded
    by the contextual-member work.
  - **Ref-field adapter follow-ups** — *parallel track* (origin/ref-system
    refinements plus bug fixes; not coupled to the API-parity sequence
    above). Internal order: fix the VM gaps (item 6, these are bugs)
    first, then the small acceptance gaps (2, 4), then the probes and
    tightenings (5), then the deep origin-channel work (1, 3). What
    remains after the landed delegated expression-origin returns,
    temp-view chaining, ctor auto-borrow, parametric-mut writes, and
    storage-annotation concreteness tightenings:
    1. Propagate write requirements through generic wrappers. A wrapper
       generic over the same origin cannot discharge a wrapped view's
       parametric-mut write; both directions reject today.
    2. Auto-borrow temporaries into `ref` constructor parameters. Only
       places auto-borrow today, while upstream also accepts temporaries.
    3. Generalize delegated-call origin expressions to argument-taking
       callees and multiple origin-binder correspondences. This needs an
       origin channel richer than the erased struct identity.
    4. Support `_`/`...` origin placeholders in applications, as suggested
       by upstream; Mojito currently rejects them.
    5. Complete the conformance probes and resulting tightenings:
       - Determine where the pin requires a qualified `Self.o` binder in
         origin clauses, then reject the bare binder in those positions.
       - Probe concreteness in return types and alias-through-return
         positions. The 2026-08-28 probes establish that an initialized
         local may use a bare generic inferred from its initializer
         (`var v: StringSlice = ...`), while a partial application such as
         `var s: Span[Int] = xs` rejects; the current storage rule matches
         that result.
       - Protect or tighten bare origin-slotted generic function parameters
         according to upstream probe results.
    6. Fix the pre-existing VM gaps exposed by this work:
       - A ref-field struct returned through a bare unbound return annotation
         loses its borrow contract and later fails with a stale-frame error,
         even if first bound to a `var`.
       - Reading a pointer field through a bare origin-generic parameter
         yields `None`.
       - Reading `var k = it.__next__().key` in value position fails for a
         heap-backed (`String`) field because `LoadPlace` copies shallowly.
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
