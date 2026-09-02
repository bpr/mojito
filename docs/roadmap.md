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

- [ ] **Native converting-constructor defaults** — lower a
  `CheckedConst::Construct` default (e.g. `arg: Optional[T] = None`, which the
  VM materializes by running the empty-Optional constructor). The pliron
  default-fill sites are scalar-oriented and reject the aggregate today
  (`checked_const_value` errors on `Construct`); emit the constructor call at
  default-fill instead (reuse `lower_call` — the `NoneType` argument is
  `LowerTy::ZeroSized`, so no physical operand).

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
  - container `Hashable` conformances on the hasher protocol
    (`List`/`Optional`/`Array`/`Set`/`Dict`/`Tuple.__hash__`; upstream
    discriminates `Optional`/`Variant` alternatives with a `UInt8` tag before
    delegating — `String`/`StringSpan` already conform)
  - the upstream-exact `Hasher` member spellings: `_update_with_simd(mut
    self, value: SIMD[_, _])` with `SIMD.to_bits()` (Mojito narrows the leaf
    to one normalized `UInt64`, which both bundled hashers mix identically),
    the keyed `AHasher[key: U256]` (Mojito's is key-less; the seeded
    initializer remains), the bytes `hash(bytes: ImmPointer[UInt8, _], n)`
    overload and `hash_seeded_bytes` (both need a pointer-backed `Span`
    constructor), and a `Span`/`as_bytes` path for `StringSpan.__hash__`
    (today it copies the bytes into a `List[Byte]`)
  - native `Variant.__hash__` dispatch (the VM feeds the discriminant then
    the active alternative; pliron reports the leaf unsupported) and CTFE
    `hash`/`default_comp_time_hasher` for compile-time dictionaries

- [ ] **Parity-unblocking infrastructure** — compiler-side features that
  each unblock several recorded parity gaps at once. These are where
  "Mojito rejects/diverges from valid Mojo" clusters today; every item
  lists what it unlocks so slices can be chosen by leverage. The remaining
  items below are in **recommended execution order**, not fan-out order. The
  ref-field adapter
  follow-ups are a **parallel track** (origin/ref-system
  refinements plus bug fixes, not coupled to the API-parity sequence) and
  are listed last with their own internal order.
  - **Temporary view arguments to parameterized static calls** — the
    2026-09-01 lift anchored loan-carrying temporary arguments
    (`b.extend(Span(a))`) for method, parameterized-method, and
    bare-identifier static calls, but the `TypeApply`/`Index`
    static-receiver lowerings (`Dict[Int, Int].fromkeys(...)`,
    `Box[String].filled(...)`) still lower arguments through bare
    `args()` with no `arg_places` and no `$arg_loan_r` anchor, so
    `Foo[T].bar(Span(a))` with no later use of `a` still traps at run
    with `vm: use after Pointer deallocation`. Lift by routing those two
    arms through `lower_call_arguments` under the same
    `call_anchors_arguments` gate (watch the place-retention change that
    swap introduces; the three transfer-effect `mir_test` pins guard
    duplicate loans).
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

### 5. Code Organization Follow-Ups *(any order — behavior-preserving)*

The 2026-09 module split (see `docs/symbol-map.md`) eliminated every source
file over 3,000 lines by moving `impl` clusters into directory submodules.
What remains needs semantic extraction, not line moves:

- [ ] **Split `expr_unconverted`** — `mir/lower_expr/expr.rs` (~2,090 lines) is
  one match over `ExprKind`; extract cohesive arm groups into `Flatten`
  methods so the dispatcher reads as a table.
- [ ] **Split `infer_method_call`** — `checker/method_calls/mc_infer.rs`
  (~1,530 lines) is a single method; extract receiver-family branches into
  helpers alongside the existing `selection`/`statics`/`builtin_types`
  siblings.
- [ ] **Split `verify_instruction`** — `mir/verify/instr.rs` (~1,320 lines) is
  one match over `MirInstr`; extract per-instruction-family check helpers.
- [ ] **Shrink the 2 kloc outlier band** — largest remaining files, all
  already responsibility-scoped, splittable further if a cohesive seam
  appears while touching them: `checker/traits.rs` (2,629),
  `mir/lower_stmt.rs` (2,595), `checker/inference.rs` (2,520),
  `checker/statements.rs` (2,471), `ast.rs` (2,464), `mir.rs` (2,426),
  `checker/type_resolution.rs` (2,395), `runtime.rs` (2,235), `checker.rs`
  (2,232 — the `Checker` struct + retained constructors/coercion helpers),
  `comptime/rewrite.rs` (2,179), `checker/declarations.rs` (2,118),
  `mir/text/write.rs` (2,116), `backend/vm/exec.rs` (2,085). Do not split
  below cohesion just to hit a size target.

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
