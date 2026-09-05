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
  what lands). The tasks below are in implementation order: prioritized by
  payoff and sorted so that every dependency precedes what needs it. A task
  closes when its bullets are done; a residue discovered inside a task moves
  to the task that owns its fix (or to task 9, the deliberate deferrals),
  never back to a finished one. Plan first: task 1 (half a page naming the
  anchor each case extends) and task 8 (ordering); the rest are direct.

  1. **View temporaries as receivers and arguments.** The one ownership
     gap the later tasks depend on (`$mat_r`/`$arg_loan_r` anchors exist;
     ownership changes have regressed before, so plan the fixture matrix).
     - A view-returning call on a temporary receiver (`String("x").strip()`)
       rejects ("reference binding to a non-place expression"): the receiver
       anchor handles bound places only.
     - A subscript view temporary as a call argument at its source's last
       use (`writer.write(s[byte=a:b])`) frees the source first:
       `$arg_loan_r` covers call/method-call temporaries only.
     - Unblocks upstream's fluent `FormatStruct(writer, "P").params(...)
       .fields(...)` (then `params` returns `ref[self] Self` again and the
       receivers go back to `self`, which needs a `Pointer[T, mutable
       origin]` deref write through a read-only `self`), and `_utils.Named`
       (a pointer-holding temporary passed as an argument reads a stale VM
       frame).
  2. **`StringSpan` parameters in upstream's shape.** Retarget the String
     API signatures from `String` to `StringSpan` (mechanical, wide): fixes
     `StringSpan` arguments not converting (today `to_string()`) and moves
     the batch-2 members (case, predicates, justification) off `String`
     only. Ride-alongs: `split(sep, maxsplit=-1)` as upstream's two
     overloads; `isspace`'s `single_character` parameter.
  3. **Hashing parity.** Decide up front whether to move the `UInt64` leaf
     to upstream's `to_bits` shape.
     - Container `Hashable` on the hasher protocol
       (`List`/`Optional`/`Array`/`Set`/`Dict.__hash__`; upstream tags
       `Optional`/`Variant` alternatives with a `UInt8` before delegating).
     - Upstream-exact `Hasher` spellings: `_update_with_simd(mut self,
       value: SIMD[_, _])` with `to_bits()`, the keyed `AHasher[key: U256]`
       (this also makes `repr(set)` byte-identical: Set prints
       `Hasher=AHasher[[0, 0, 0, 0] : SIMD[DType.uint64, 4]]` upstream),
       the bytes `hash(bytes: ImmPointer[UInt8, _], n)` overload and
       `hash_seeded_bytes`.
     - CTFE `hash`/`default_comp_time_hasher` for compile-time dictionaries.
     - Raw seam only: a generic body forwarding its own `H` into `hash[H](x)`
       with no caller binding falls back to the declaration default.
  4. **Optional, Tuple, and Slice odds and ends.** Self-contained.
     - The raising `opt[]` subscript: empty-subscript form on nominal
       receivers plus `EmptyOptionalError`'s `TypeNames` text.
     - `Tuple`/`Array` `Defaultable`: needs `Ts[i]()`/`Self.T()` element
       default construction.
     - Tuple's static `__len__()` cannot coexist with the instance one under
       arity-keyed selection (re-probe after 2026-09-05's same-arity operand
       selection).
     - An explicit dunder call other than comparisons/`__len__` on a Tuple
       whose specialization is not yet minted (`t.__contains__(x)`) reports
       no such method; the operator spelling works.
     - `Slice(...)` reads only `Int`/`None` argument expressions; an
       `Optional[Int]` variable needs the nominal slot read.
     - Explicit `.write_to(writer)` on a slice descriptor is not wired.
  5. **String Unicode, iterator, and parsing extras.** Port on demand.
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
  6. **Origin-bearing `Span`/`Pointer` construction.**
     - `Span(unsafe_ptr=, length=)` takes `Pointer[T, MutUntrackedOrigin]`:
       a `Pointer[T, origin]` parameter cannot bind through a constructor
       type application (`Span[Byte, origin_of(self)](...)` rejects with a
       type-argument count).
     - `Pointer`'s `[unsafe_offset=]` keyword subscript (upstream deprecates
       the positional form).
     - An origin-bearing struct return in a free function (`def view[T](ref
       xs: List[T]) -> Span[T, origin_of(xs)]`) rejects with a type-argument
       count though the same spelling works in a method — this keeps
       `reversed(list)` a method spelling (`xs.__reversed__()`).
     - `var it = xs.__iter__()` cannot infer the local's origin parameter
       (`failed to infer parameter 'iterable_origin'`); construct from a
       `ref` explicitly.
  7. **Storage-shape items with a known blocker.** Short plan (ordering).
     - Relax K/V/element bounds toward upstream's Movable-only `KeyElement`
       (a per-API `where` pass, as `List[T: AnyType]`).
     - `(*, unsafe_uninit_length)` construction and resize: blocked on an
       uninit-element storage story for List (MaybeUninit-adjacent).
  8. **Native-lane follow-ups.** The VM runs all of these; monomorphizer
     and native-drop bug fixes.
     - `repr` lowers natively only for Strings, without escapes; no user
       `write_repr_to` runs natively, so the repr family (task 1's texts,
       `FormatStruct`) is pinned by conformance fixtures only.
     - Monomorphization cannot resolve a method-level parameter for a
       parameterized `@staticmethod` through a non-variadic generic instance
       (`p.pick[Int]()`) or for a method whose only parameter is
       callable-bounded (`__getitem__[F: def() -> Int]` in
       conformance/fixtures/subscript_call_contracts.mojo).
     - conformance/fixtures/tuple_consume_elements.mojo prints correctly,
       then traps `use after Pointer deallocation` at teardown (also on the
       da4d129 baseline): a native drop gap.
     - Projection tag-mismatch trap categories differ (VM `TypeError`
       `Variant holds 'Int', not 'String'`; native `UnhandledError`), so no
       error-differential fixture pins it.
     - An erased body forwarding its own constructible binder
       (`hash[Self.H](key)` in `DictEntry.__init__`) monomorphizes with the
       callee's default: native `Dict[K, V, SumHasher]` entries hash under
       `AHasher` while the VM honors `SumHasher`. Fixture outputs agree
       because each lane is self-consistent; a probe printing from the hasher
       diverges.
     - The generic `next[T: Iterator](mut it: T)` body fails natively
       (`unsupported reference-result method adapter`); `next` is pinned by
       conformance/fixtures/next_builtin.mojo only.
  9. **Deliberate deferrals from the generic-clone arc.** Nothing depends
     on these; each is a conscious limit, listed here so it is not mistaken
     for unfinished task work.
     - Clones are minted per whole instance (no reachability pruning) and
       re-checked each discovery round: `benchmarks/compile/stdlib_heavy` is
       about 2.2x its pre-clone baseline in release (`docs/performance.md`);
       the repr methods added 2026-09-05 cost about 5% in debug across the
       compile benchmarks. Lever: reachability-pruned minting.
     - An instantiation whose argument mentions `StringLiteral` (`{"a": 1}`
       is `Dict[StringLiteral, Int]`) keeps the erased path: its values keep
       the literal runtime representation while an un-annotated binding
       materializes `String`. Lifting it needs one runtime representation
       for `StringLiteral` and `String`, or typing the display as
       `Dict[String, Int]` as Mojo does.
     - A call inside an unstamped bundled body on a bundled struct's generic
       method keeps the erased path (requests are admitted from user code,
       clone bodies, variadic specs, instances, and user structs).
     - An instance clone whose walk cannot resolve an application (a
       variadic template over a nested public `Tuple` argument) is dropped
       to the erased path rather than failing the program.
  10. **Diagnostic wording and strictness.**
      - An unavailable where-gated method reports `'set' is unavailable for
        Variant[Conn]: its where clause evaluated to False` rather than
        upstream's clause text.
      - Mojito accepts a bare `T` in a struct field type and a
        non-`Deinitable` field parameter (`struct Box[T: Copyable &
        Movable]: var value: T`) where upstream requires `Self.T` and a
        `Deinitable` bound.

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
