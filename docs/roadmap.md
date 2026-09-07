# Mojito Roadmap

The single task tracker: an ordered checklist of **unfinished** work only.
Completed work leaves this file — the supported surface lives in
[`docs/features.md`](features.md), user-visible history in
[`CHANGELOG.md`](../CHANGELOG.md), and lasting design invariants in
[`docs/architecture.md`](architecture.md). North star: self-hosting — prefer
the smallest honest language change that unlocks a real library pattern, with
positive and negative tests.

Sections and their checkboxes are in implementation order; the first unchecked
box is the default next task. *(recurring)* and *(any order)* sections are
exempt from the ordering.

## Ordered Work

### 1. Native Backend: Pliron Stage 6 — Optimization and Distribution

- [ ] **Dependency-upgrade rehearsal** — the one open Stage 6 acceptance
  item. Blocked on the first upstream pliron/llvm-sys release newer than the
  pins (0.17.0 / llvm-sys 221.0.1). Procedure and evidence:
  `docs/notes/pliron-stage6.md`.
- [ ] **Promotion decision** — promote Pliron from experimental only with:
  - semantic parity on all runnable corpus and conformance cases, no
    untracked MIR gaps;
  - reproducible tooling on the supported Linux target, acceptable
    benchmarks, no broad upstream fork;
  - one successful upgrade rehearsal and a sustained regression-free gate
    period (accruing in the stage 6 note).
  - Blockers: disproportionate upstream churn, missing dialect/export
    coverage, irreproducible LLVM discovery, semantic drift in
    output/errors/references/drop order, a Mojito dialect duplicating MIR,
    runtime/codegen layout disagreement, optimization-only miscompiles,
    undocumented ABI lock-in, stale adoption evidence.
  - Close with a `docs/notes/` decision record citing evidence per
    criterion. Promotion never removes the VM or makes Pliron a required
    compiler layer.
- [ ] **Cranelift fallback** *(only on material Pliron failure)* — record
  the evidence, then implement the same acceptance slices over verified MIR
  with the shared target/layout/runtime ABI and differential corpus. Not a
  parallel second backend. If it also fails: reassess direct LLVM, Melior,
  Inkwell, or a C/C++ source backend with a fresh record.
- [ ] **Native SIMD lowering** — after language parity, replace the
  lane-by-lane memory computation with LLVM fixed-vector SSA
  (`docs/notes/native-simd-pliron-assessment.md`), keeping the storage/call
  ABI and width-one scalars scalar. Order:
  1. construction/splat and aggregate↔vector boundary conversion;
  2. elementwise arithmetic, bitwise, comparisons, mask select;
  3. checked dynamic extract/insert, compile-time shuffle;
  4. casts (VM-exact wrapping, Float32 rounding, saturating float→int);
  5. reductions (signedness, NaN, deterministic float order).
  - Bounds-check dynamic lane indexes before LLVM ops; convert `<N x i1>`
    masks to byte-lane storage only at boundaries.
  - Acceptance: VM/native differentials at `O0` and release per dtype,
    width, op, conversion, shuffle, reduction; sanitizer-clean crossings;
    unchanged `LayoutCx`/`docs/native-abi.md` (or an ABI-versioned
    revision); target-code inspection showing vector instructions.
- [ ] **Native generic-holder temporaries with heap-owning implicitly
  copyable fields** — `GenericHolder[Box](Box(5))` or Dict's
  `DictEntry[Optional[Int], V]` appended to a `List` reads freed memory
  natively (VM correct). Keeps `Dict[Optional[Int], _]` VM-only
  (`conformance/fixtures/optional_dict_keys.mojo` stays out of `assets/ok`).
  Bisect the `var` field-move of the copy-lifecycle value inside the generic
  constructor against the owned-temp marking in
  `crates/mojito-pliron/src/lower/calls.rs`.
- [ ] **Native converting-constructor defaults** — lower a
  `CheckedConst::Construct` default (`arg: Optional[T] = None`); the
  default-fill sites reject the aggregate (`checked_const_value` errors on
  `Construct`). Emit the constructor call at default-fill via `lower_call`
  (the `NoneType` argument is `LowerTy::ZeroSized`).

### 2. Catch Up To Current Mojo *(recurring — reopens at every nightly re-pin)*

- When the pinned nightly moves: re-pin [`docs/mojo-nightly.md`](mojo-nightly.md),
  re-probe [`conformance/parity.tsv`](../conformance/parity.tsv), burn down
  the divergences (`parity.tsv` notes; `mojito-only`/`mojo-only` rows of
  `conformance/cases.tsv`).
- Rule: Mojito matches or subsets Mojo — extensions only as temporary
  bridges tracking upstream deprecations or cited upstream-roadmap features,
  re-probed at every re-pin.
- The `a79fbdf59f2` pass (2026-08-26, Mojo `1.1.0.dev2026082605`) is
  complete (`docs/mojo-nightly.md`); the next re-pin recreates this
  section's checkbox.

### 3. Grow The CPU Standard Library *(demand-first)*

- [ ] **Collection API parity** — grow the tuple, slice, optional/variant,
  and String surfaces toward the audited head (`docs/features.md` records
  what lands). The tasks below are in impact order: soundness of the
  executable oracle first, then everyday spellings that reject today, then
  parity details; no task depends on a later one. A
  task closes when its bullets are done; a residue discovered inside a task
  moves to the task that owns its fix (or to task 5, the deliberate
  deferrals), never back to a finished one. Plan first: task 1 (ordering);
  the rest are direct.

  1. **Storage-shape items with a known blocker.** Short plan (ordering).
     - Relax K/V/element bounds toward upstream's Movable-only `KeyElement`
       (a per-API `where` pass, as `List[T: AnyType]`).
     - `(*, unsafe_uninit_length)` construction and resize: blocked on an
       uninit-element storage story for List (MaybeUninit-adjacent).
  2. **String Unicode, iterator, and parsing extras.** Port on demand.
     - `upper`/`lower`: simple-case subset (ASCII, Latin-1, Latin
       Extended-A, Greek, Cyrillic, `ß` → `SS`); upstream ships full Unicode
       simple and special casing tables.
     - `count_codepoints`/`count_graphemes` `raise` on invalid UTF-8
       (upstream: non-raising) and decode through an eager `to_string()`
       copy per step.
     - Missing: `codepoint_slices_reversed`, `graphemes_reversed`,
       `__reversed__`, `bytes()`, `split_at_grapheme`, `peek_next`.
     - `atof` is correctly rounded only while the significand (≤19 digits)
       and power of ten (≤22) stay exact; NaN prints `NaN` (upstream `nan`).
     - `len(s)` on a String or StringSpan is accepted (byte length) where
       upstream rejects it as ambiguous (`byte_length()`, `len(s.codepoints())`,
       `len(s.graphemes())`); an extension to drop at the next re-pin.
  3. **Compile-time collections.** `comptime d = {...}` Dict/Set values
     hashed with `default_comp_time_hasher`. `CtValue` has no mapping kind
     (`crates/mojito-types/src/ct.rs`), `Elab::eval` has no dict-display
     arm, and the VM-CTFE purity walk admits only the hasher protocol's
     method calls, so a general compile-time method-call rule comes with it.
  4. **Diagnostic wording and strictness.**
     - An unavailable where-gated method reports `'set' is unavailable for
       Variant[Conn]: its where clause evaluated to False` rather than
       upstream's clause text.
     - A bare `T` in a struct field type (`var value: T`) is rejected only
       by the specialization conformance oracle (`unknown type 'T'`) and a
       non-`Deinitable` field parameter (`struct Box[T: Copyable & Movable]:
       var value: Self.T`) is accepted, where upstream reports `use Self.T`
       and requires a `Deinitable` bound.
     - A struct's own value parameter spelled bare (`n` rather than
       `Self.n`) in an expression-position bracket slot reaches MIR as an
       untyped `UseVar`; upstream rejects the bare spelling ("use
       `Self.n`"), so it should be a checker error.
     - `Counter[Self.index]` (a field, not a parameter) in a bracket slot
       reports `not a compile-time Int constant: Counter` rather than
       naming the field.
  5. **Deliberate deferrals.** Nothing depends on these; each is a
     conscious limit, listed here so it is not mistaken for unfinished
     task work.
     - Clones are minted per whole instance (no reachability pruning) and
       re-checked each discovery round: `benchmarks/compile/stdlib_heavy`
       is about 2.2x its pre-clone baseline in release
       (`docs/performance.md`); the repr methods added 2026-09-05 cost
       about 5% in debug across the compile benchmarks. Lever:
       reachability-pruned minting.
     - An instantiation whose argument mentions `StringLiteral` (`{"a": 1}`
       is `Dict[StringLiteral, Int]`) keeps the erased path: its values
       keep the literal runtime representation while an un-annotated
       binding materializes `String`. Lifting it needs one runtime
       representation for `StringLiteral` and `String`, or typing the
       display as `Dict[String, Int]` as Mojo does.
     - A call inside an unstamped bundled body on a bundled struct's
       generic method keeps the erased path (requests are admitted from
       user code, clone bodies, variadic specs, instances, and user
       structs).
     - An instance clone whose walk cannot resolve an application (a
       variadic template over a nested public `Tuple` argument) is dropped
       to the erased path rather than failing the program.
     - Casting a tracked pointer to an untracked origin at its source's
       last use (`s.unsafe_ptr().unsafe_origin_cast[ImmUntrackedOrigin]()`)
       frees the source before the pointer is read — as upstream would.
     - Type-pack calls inside a nested `def` and whole-pack-forwarded calls
       keep the syntactic element-typing path (`a heterogeneous pack
       specialization needs an expression whose type is statically evident
       before checking` for `take(b, 1)` over a local `b`); top-level calls
       consult the checker's instantiation. Lifting it needs four pieces:
       nested templates live only in `NestedMono` and are deleted by
       `replace_templates` before the checker sees them, the checker would
       have to accept a nested variadic shell abstractly,
       `def_specialization_requests` is top-level only, and
       `Mono.def_call_targets` must reach `NestedMono::scan_expression`
       with a `def_request_target` fallback.
     - A user variadic struct application as a pack element
       (`Tuple[TypeNames[Int]]`) keeps the fixed-arity diagnostic: its
       erased shell has no sound nominal form (the public `Tuple` is the one
       compiler-known template whose `*Ts` absorbs every argument).
     - A standalone nullary vector construction `SIMD[d, w]()` rejects
       (`SIMD construction expects w element(s) or 1 to splat, got 0`);
       only a Tuple element defaults to zero lanes.
     - A public Tuple as an explicit type argument does not conform to
       `Deinitable` (`make[T: Defaultable & Deinitable]()` over
       `Tuple[Int, Bool]` reports the bound failure) while the inferred
       shape runs.
     - Value-parameterized structs get no instance clones, so an erased
       body's `_unqualified_type_name[Self.T]()` spells `T`
       (`repr([1, 2])` prints `Array[T, 2]([Int(1), Int(2)])` where
       upstream prints the element type), and a `Self.n` bracket argument
       inside such a body (`Counter[Self.length](i)`) is VM-only: native
       monomorphization needs a compile-time-constant value argument
       (`unsupported value parameter 'length' is not compile-time
       constant`).
     - `_unqualified_type_name` spells nested structs unqualified where
       upstream keeps a non-prelude struct's module path
       (`Optional[std.collections.dict.Dict[...]]`, `List[up.Flag[True]]`)
       while `List`/`Optional`/`String`/`SIMD` stay bare.
     - A non-parameterized struct alias mentioning a value parameter
       (`comptime Alias = S[Self.T, Self.length]`) resolved through an
       instance (`S[Int, 3].Alias`) keeps `length` symbolic (`expected
       S[Int, length], found S[Int, 3]`): `associated_type_from_base`
       substitutes type parameters only. Parameterized aliases substitute
       both.

- [ ] **Filesystem and I/O slice** — representative file/path/stream APIs
  on the Writer and explicit-destroy foundations.
- [ ] **Time, random, and testing slices** — deterministic testable cores;
  host-dependent behavior behind runtime services.

### 4. Packaging, Artifacts, And Developer Tooling *(any order unless noted)*

- [ ] **Compile-time performance** — Hello World is 0.8 s release / 4.6 s
  debug (`docs/performance.md`; was 24 s / 52 s before the shared
  `Arc<CheckedTables>`). Next, in order:
  1. avoid the redundant checker passes (Hello World re-elaborates and
     re-checks once because the request scan always finds the prelude's own
     Tuple/def requests, and every check runs two transfer rounds);
  2. `checked_var_types` scans the whole expression table per variable and
     `explicit_destroy` re-derives deinitability per struct per pass;
  3. only then cache the elaborated/checked stdlib across processes.
- [ ] **Feature and target options** — checked CLI/build configuration
  recorded in artifacts and diagnostics.
- [ ] **Compiled package artifacts** — a versioned `.mojoc` representation
  (modules stay non-first-class). Per-directory resolution order:
  1. source package
  2. `.mojoc`
  3. source module
  4. legacy `.mojopkg`
- [ ] **Debugging metadata and inspection** — stack/source diagnostics, MIR
  inspection, debugger-oriented value rendering.
- [ ] **Testing tools** — Mojito-native assertions, expected-error tests,
  differential-harness integration.
- [ ] **Distribution reproducibility gate** *(last)* — the release check
  rebuilds, tests, documents, and reproduces conformance from the crates.io
  archive alone.

### 5. Code Organization Follow-Ups *(any order — behavior-preserving)*

The 2026-09 module split (`docs/symbol-map.md`) removed every file over
3,000 lines. What remains needs semantic extraction, not line moves:

- [ ] **Split `expr_unconverted`** — `mir/lower_expr/expr.rs` (~2,090 lines)
  is one match over `ExprKind`; extract arm groups into `Flatten` methods.
- [ ] **Split `infer_method_call`** — `checker/method_calls/mc_infer.rs`
  (~1,530 lines) is one method; extract receiver-family branches beside
  `selection`/`statics`/`builtin_types`.
- [ ] **Split `verify_instruction`** — `mir/verify/instr.rs` (~1,320 lines)
  is one match over `MirInstr`; extract per-family check helpers.
- [ ] **Shrink the 2 kloc band** — split further only along a cohesive
  seam while touching them: `checker/traits.rs` (2,629),
  `mir/lower_stmt.rs` (2,595), `checker/inference.rs` (2,520),
  `checker/statements.rs` (2,471), `ast.rs` (2,464), `mir.rs` (2,426),
  `checker/type_resolution.rs` (2,395), `runtime.rs` (2,235), `checker.rs`
  (2,232), `comptime/rewrite.rs` (2,179), `checker/declarations.rs`
  (2,118), `mir/text/write.rs` (2,116), `backend/vm/exec.rs` (2,085).

## Task Lifecycle Policy

`roadmap.md` is the only task list: no parallel todo file, no retained
completed tasks.

- Unfinished work is an unchecked, outcome-oriented task in **Ordered
  Work**; design detail lives in plans or `docs/notes/`.
- A task is complete only when implementation, focused positive and negative
  coverage, documentation, and `scripts/check` agree. In the same change,
  delete it here and record the outcome in `docs/features.md`,
  `CHANGELOG.md`, and (for design invariants) `docs/architecture.md`.
- Rewrite partially completed tasks so only the remaining outcome is stated;
  prefer one checkbox per independently demonstrable outcome.

## Working Rule

1. Start with a self-hosted library or small user-facing acceptance case.
2. Record the current failure with a focused test.
3. Implement the smallest compiler change that makes the program honest.
4. Add positive and negative coverage at the owning compiler phase.
5. Run `scripts/check` before marking the task complete.
