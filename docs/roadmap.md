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
  language parity, replace Pliron's current memory-resident, lane-by-lane SIMD
  computation with LLVM fixed-vector SSA operations while preserving the
  existing target/layout/runtime ABI at storage and call boundaries. The
  capability analysis and design rationale live in
  `docs/notes/native-simd-pliron-assessment.md`. Keep width-one scalar aliases
  scalar; use `<N x lane>` only as the compute form,
  allowing LLVM to legalize values wider than a physical target register and
  retaining the scalar-aggregate path as the defined fallback when a target or
  operation cannot use vectors. Implement and demonstrate the slice in this
  order:
  1. construction/splat and aggregate↔vector boundary conversion;
  2. elementwise arithmetic, bitwise operations, comparisons, and mask select;
  3. checked dynamic extract/insert and compile-time shuffle;
  4. casts, including the VM-exact wrapping, Float32 rounding, and saturating
     float-to-integer-before-rewrap behavior; and
  5. reductions, preserving signedness, NaN behavior, and deterministic
     floating reduction order unless the language contract explicitly permits
     reassociation.

  Pliron 0.17 already exposes fixed/scalable `VectorType`, vector-capable LLVM
  arithmetic/comparison/cast/select operations, `InsertElementOp`,
  `ExtractElementOp`, `ShuffleVectorOp`, and `CallIntrinsicOp` for
  `llvm.vector.reduce.*`; use fixed vectors because Mojito widths are
  compile-time source facts. Bounds-check dynamic lane indexes before LLVM
  operations, whose out-of-range result may be poison, and convert LLVM
  `<N x i1>` masks to the existing byte-lane storage form only at boundaries.
  Acceptance requires VM/native differential coverage at `O0` and release for
  every dtype, supported width, operation, edge conversion, shuffle, and
  reduction; sanitizer-clean storage/ABI crossings; unchanged `LayoutCx` and
  `docs/native-abi.md` contracts (or an explicit ABI-versioned revision); and
  target-code inspection proving representative supported cases select vector
  instructions rather than merely producing legal vector IR.

- [ ] **Native generic-holder temporaries with heap-owning implicitly
  copyable fields** — a generic struct temporary whose field is an
  `ImplicitlyCopyable` heap-owning struct (`GenericHolder[Box](Box(5))`,
  Dict's `DictEntry[Optional[Int], V]`) appended to a `List` reads freed
  memory natively (`use after Pointer deallocation`; the VM is correct; a
  non-generic holder and `List[Optional[Int]]` are fine). Consequence:
  `Dict[Optional[Int], _]` and `Dict[_, Optional[Int]]` are VM-only today
  (`conformance/fixtures/optional_dict_keys.mojo` is kept out of
  `assets/ok`). Bisect the `var` field-move of the copy-lifecycle value inside
  the generic constructor against the owned-temp marking in
  `crates/mojito-pliron/src/lower/calls.rs`.
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

- [ ] **Collection API parity** — grow the tuple, slice, optional/variant,
  and String surfaces toward the audited head, one session-sized slice at a
  time (`docs/features.md` records what lands; the self-hosted `Variant`
  landed 2026-09-04). What remains is listed below as tasks, ordered by
  expected payoff: the first task unlocks the widest family of features,
  and the later ones progressively narrower ones. Each task names the
  residues it closes, so finishing one deletes its bullet.
  Each task also says whether to `/plan` first: most are direct fixes;
  tasks 1, 5, and 9 carry a design decision and get a plan (1 and 5 share
  one), and tasks 2, 3, and 4 need only a half-page list of the sites or
  the one choice named in their note. Tasks closed 2026-09-05 (everyday
  spellings; same-arity dunder overloads) are deleted, not listed.

  1. **Per-instantiation generic struct bodies: the remaining family.**
     Ordinary generic structs now get per-instantiation method clones
     (landed 2026-09-04: `docs/features.md` Generics row; design in
     `docs/architecture.md`), which fixes `_unqualified_type_name[Self]` and
     `comptime if Self.T` inside clones. What is still open, in order:
     - Per-call specialization of method-level type parameters on every
       generic struct (`Optional.map[U]`, `__hash__[H]`): the discovery gate
       in `src/compiler.rs` still admits variadic owners only, and a clone
       inside an instance must compose the instance key with the call's.
     - Upstream's `FormatStruct(writer, "Name").params(...).fields(...)`
       builder needs variadic-pack methods on a struct (`def fields[*Ts:
       Writable](self, *args: *Ts)`: the elaborator reports "'Ts' is not a
       compile-time type" for a method-level pack while the same shape on a
       free def specializes) and a `ref` field to the writer.
     - `write_repr_to` on `Optional` and `List`/`Dict`/`Set` repr in
       upstream's text (`List[SIMD[DType.int, 1]]([Int(1), Int(2)])`) instead
       of the field-wise `Name(field=value)` fallback; `List`/`Set` bodies
       spell `TypeNames[Self.T]()`, a variadic struct over a symbolic
       argument inside the retained template body, which task 5 must first
       tolerate.
     - `OptionalReg` in subset shape (`Boolable`/`Defaultable`/
       `TrivialRegisterPassable`, no device members). `Optional.copied` is
       not ported: upstream spells it with a legacy custom self type
       (`@__allow_legacy_custom_self_type`).
     - Cost: clones are minted per whole instance (every available method,
       no reachability pruning) and each discovery round re-checks them;
       `benchmarks/compile/stdlib_heavy` costs 2.2x its baseline
       (`docs/performance.md`), one extra round of which comes from an
       inferred generic-def call inside a clone body
       — a genuinely new `hash[String, AHasher]` instantiation inside the
       `Dict` clone, which the erased template never requested; an
       occurrence whose def clone already exists retargets without a round).
       Structs with value parameters
       (`Array[T, length]`), origin binders (`Span`, the iterator structs),
       or callable-bounded parameters keep the erased path, and so does an
       instantiation whose argument mentions `StringLiteral` (`{"a": 1}`
       is `Dict[StringLiteral, Int]`): its values keep the literal runtime
       representation while an un-annotated binding of the type
       materializes `String`, so a clone body cannot type against its own
       `Self.T`. Lifting that means giving `StringLiteral` one runtime
       representation with `String` (or typing the display as
       `Dict[String, Int]`, as Mojo does).
     Related but separate: the bundled `Writable` trait declares only
     `write_to`, so a bounded `T: Writable` cannot call `write_repr_to`
     (spell `repr(x)`); and upstream spells
     `_unqualified_type_name[Tuple[Int, Bool]]()` as
     `Tuple[<unprintable>, {}]`, so shared fixtures avoid tuple type names.

  2. **View temporaries as receivers and arguments.** Two ownership-anchor
     gaps make idiomatic String chains fail. Short plan (half a page): the
     anchors already exist (`$mat_r`, `$arg_loan_r`), so the plan only
     names the anchor each case extends and the fixture matrix, because
     ownership changes have regressed before.
     - A view-returning call on a temporary receiver (`String("x").strip()`)
       rejects with "reference binding to a non-place expression" because
       the receiver anchor only handles bound places. Workaround: bind the
       receiver first.
     - A subscript view temporary passed as a call argument at its source's
       last use (`writer.write(s[byte=a:b])`) frees the source first. The
       `$arg_loan_r` anchor covers call and method-call temporaries only.
       Workaround: bind the view to a local.

  3. **`StringSpan` parameters in upstream's shape.** Upstream's String
     APIs take `StringSpan`; Mojito's take `String`, so a `StringSpan`
     argument does not convert (today: `to_string()`), and the batch-2
     members (case, predicates, justification) live on `String` only.
     Retargeting the signatures fixes both at once. No design; the plan is
     the list of signatures to retarget, since the change is mechanical but
     wide across the stdlib and its call sites. Cosmetic siblings that
     can ride along: `split(sep, maxsplit=-1)` is one defaulted overload
     where upstream declares two (call sites are identical), and `isspace`
     drops upstream's `single_character` parameter.

  4. **Hashing parity.** Independent of the above, one slice. The one
     decision to make up front is whether to move the `UInt64` leaf to
     upstream's `to_bits` shape (second bullet); the rest is porting and
     needs no plan.
     - Container `Hashable` conformances on the hasher protocol
       (`List`/`Optional`/`Array`/`Set`/`Dict.__hash__`; upstream
       discriminates `Optional`/`Variant` alternatives with a `UInt8` tag
       before delegating — `String`/`StringSpan`/`Tuple` already conform).
     - Upstream-exact `Hasher` member spellings: `_update_with_simd(mut
       self, value: SIMD[_, _])` with `SIMD.to_bits()` (Mojito narrows the
       leaf to one normalized `UInt64`, which both bundled hashers mix
       identically, so both backends' `UInt64` Variant discriminant lane is
       bit-identical to upstream's `UInt8` tag until this lands), the keyed
       `AHasher[key: U256]` (Mojito's is key-less; the seeded initializer
       remains), and the bytes `hash(bytes: ImmPointer[UInt8, _], n)`
       overload plus `hash_seeded_bytes` (the pointer-backed
       `Span(unsafe_ptr=, length=)` constructor they need landed 2026-09;
       `StringSpan.__hash__` already feeds `as_bytes()` to
       `_update_with_bytes` without copying).
     - CTFE `hash`/`default_comp_time_hasher` for compile-time dictionaries.
     - On the raw seam only, a generic body forwarding its own `H` into
       `hash[H](x)` without specialization reifies the binder's name, and the
       VM's `ConstructTypeParam` falls back to the declaration default
       instead of the caller's binding. The compiled discovery path
       specializes the clone and is unaffected.

  5. **Variadic pack forwarding through generic defs.** `Variant[*Ts]` or
     `Variant[T, String]` applied to an enclosing `def f[*Ts]`/`def f[T]`'s
     own parameters cannot check: the erased template body resolves no
     variadic template application (user variadic structs never could; the
     retired intrinsic could). Pinned by tests/compiler_test.rs
     `variant_pack_forwarding_through_a_generic_def_is_rejected`. Needs
     variadic template applications over symbolic arguments to resolve in
     erased template bodies, and struct specialization requests discovered
     from substituted generic signatures. Plan it inside task 1's plan (same
     discovery machinery) and sequence it after 1.

  6. **Optional, Tuple, and Slice odds and ends.** Small items, each
     self-contained; no plan:
     - The raising `opt[]` subscript needs the empty-subscript form on
       nominal receivers plus `EmptyOptionalError`'s `TypeNames` text.
     - `Tuple`/`Array` `Defaultable` needs `Ts[i]()`/`Self.T()` element
       default construction (one shared blocker).
     - Tuple's static `__len__()` overload cannot coexist with the instance
       one under arity-keyed dunder selection (same-arity operand selection
       landed 2026-09-05; re-probe whether the static/instance pair now
       coexists).
     - An explicit dunder call other than the comparisons and `__len__` on a
       Tuple whose specialization discovery has not yet minted
       (`t.__contains__(x)`) reports no such method; the operator spelling
       (`x in t`) works.
     - `Slice(...)` construction reads only `Int`/`None` argument
       expressions (`infer_slice_construction`); an `Optional[Int]`-typed
       variable needs the nominal Optional's slot read.
     - The explicit `.write_to(writer)` spelling on a slice descriptor is
       not wired (print, `String(x)`, and `Writer.write` are).

  7. **String Unicode, iterator, and parsing extras.** Port when a fixture
     needs them; no plan:
     - `upper`/`lower` cover a simple-case subset (ASCII, Latin-1, Latin
       Extended-A, Greek, Cyrillic, `ß` -> `SS`); upstream ships the full
       Unicode simple and special casing tables (`_unicode.mojo`, generated
       lookups).
     - `count_codepoints`/`count_graphemes` still `raise` on invalid UTF-8
       (upstream's are non-raising over trusted buffers) and decode through
       an eager `to_string()` copy per step, which the iterators' `__len__`
       inherits.
     - Missing iterator members: `codepoint_slices_reversed`,
       `graphemes_reversed`, `__reversed__`, `bytes()`, `split_at_grapheme`,
       and `peek_next`.
     - `atof` is correctly rounded only while the significand (at most 19
       digits) and the power of ten (at most 22) stay exact, and Mojito
       prints NaN as `NaN` where upstream prints `nan`.

  8. **Origin-bearing `Span`/`Pointer` construction.** `Span(unsafe_ptr=,
      length=)` takes a `Pointer[T, MutUntrackedOrigin]` because a
      `Pointer[T, origin]` parameter cannot bind through a constructor type
      application: Pointer parameters coerce only on exact origins, and the
      constructor path resolves type applications without origin slots, so
      `Span[Byte, origin_of(self)](...)` rejects with a type-argument count.
      `Pointer`'s `[unsafe_offset=]` keyword subscript (upstream deprecates
      the positional form) belongs to the same slice. Two more origin gaps
      found 2026-09-05: an origin-bearing struct return type in a free
      function (`def view[T](ref xs: List[T]) -> Span[T, origin_of(xs)]`)
      rejects with a type-argument count although the same spelling works
      in a method — this is what keeps `reversed(list)` a method spelling
      (`for x in xs.__reversed__()`) rather than upstream's free function —
      and `var it = xs.__iter__()` cannot infer the local's origin parameter
      (`failed to infer parameter 'iterable_origin'`), so a borrowed
      iterator local must be constructed explicitly from a `ref`. No plan;
      one constructor-path fix, the free-function signature path, the
      local-binding inference, and a subscript spelling.

  9. **Storage-shape items with a known blocker.** Short plan, mostly to
      fix the order: the uninit-element storage story is a design question,
      and the bounds relaxation rewrites many signatures at once.
      - Relaxing K/V/element bounds toward upstream's Movable-only
        `KeyElement` (a List-`AnyType`-style per-API `where` pass).
      - `(*, unsafe_uninit_length)` construction and resize, blocked on an
        uninit-element storage story for List (MaybeUninit-adjacent).

  10. **Native-lane follow-ups** (the VM runs all of these; they belong to
      the native backend but are recorded here because collection fixtures
      surface them). No plan: these are monomorphizer and native-drop bug
      fixes; the teardown trap needs a debugging session, not a design.
      - `repr` lowers natively only for Strings and without escapes; no user
        struct's `write_repr_to` runs natively, so `repr(v)` on a Variant
        waits on the general native repr path.
      - Monomorphization cannot resolve a method-level parameter for a
        parameterized `@staticmethod` reached through an instance of a
        non-variadic generic struct (`p.pick[Int]()`: `cannot resolve
        parameter T`) or for an instance method whose only parameter is
        callable-bounded (`__getitem__[F: def() -> Int](self, callback: F)`
        in conformance/fixtures/subscript_call_contracts.mojo: `cannot
        resolve parameter F`).
      - conformance/fixtures/tuple_consume_elements.mojo
        (`values^.consume_elements[print_length]()` over `Array` elements)
        prints correctly and then traps `use after Pointer deallocation` at
        teardown, on both the current tree and the da4d129 baseline: a
        pre-existing native drop gap, not a manifest row.
      - The projection tag-mismatch trap categories differ (the VM raises a
        `TypeError` `Variant holds 'Int', not 'String'` where native traps
        `UnhandledError`), so no error-differential fixture pins it.
      - An erased body forwarding its own constructible binder
        (`hash[Self.H](key)` in `DictEntry.__init__`, or any
        `Holder[H: Hasher]` constructor) monomorphizes with the callee's
        declaration default: native `Dict[K, V, SumHasher]` entries hash
        under `AHasher` while the VM honors `SumHasher` (the binder spelling
        the MIR reifies, `Const::Str("H")`, resolves only on the VM).
        Fixture outputs agree because both lanes are self-consistent; a
        probe printing from the hasher diverges.
      - The generic `next[T: Iterator](mut it: T)` body fails natively
        (`unsupported reference-result method adapter` on its erased
        trait-bound `__next__` call, instantiated for a range), so `next`
        is pinned by a conformance-only fixture
        (conformance/fixtures/next_builtin.mojo); the VM runs it.

  11. **Diagnostic wording and strictness.** No plan.
      - An unavailable where-gated method reports Mojito's `'set' is
        unavailable for Variant[Conn]: its where clause evaluated to False`
        rather than upstream's clause text.
      - Mojito accepts a bare `T` in a struct field type and a
        non-`Deinitable` field parameter (`struct Box[T: Copyable & Movable]:
        var value: T`) where upstream requires `Self.T` and a `Deinitable`
        bound (a lenience, not a rejection).

- [ ] **Filesystem and I/O slice** — port representative file/path/stream APIs on
  the Writer and explicit-destroy foundations.
- [ ] **Time, random, and testing slices** — add deterministic testable cores and
  isolate host-dependent behavior behind runtime services.

### 4. Packaging, Artifacts, And Developer Tooling *(any order unless noted)*

- [ ] **Compile-time performance** — Hello World is 0.8 s release / 4.6 s
  debug after the 2026-09-04 fix that shares one `Arc<CheckedTables>`
  across all function lowerings (`docs/performance.md` §Measurements; it
  was 24 s / 52 s). The release profile is now flat: four checker passes at
  0.09 s each (two discovery rounds × two transfer rounds), `explicit_destroy`
  0.06 s per check, MIR lowering 0.12 s, drop elaboration 0.07 s. Next steps,
  in order: (1) avoid the redundant checker passes — Hello World needs no
  specialization yet re-elaborates and re-checks once because the request
  scan always finds the prelude's own Tuple/def requests, and every check
  runs two transfer rounds; (2) `checked_var_types` still scans the whole
  expression table per variable and `explicit_destroy` re-derives
  deinitability per struct per pass; (3) only then consider caching the
  elaborated/checked stdlib across processes. This also shrinks every
  corpus/test process and the nightly gate.
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
