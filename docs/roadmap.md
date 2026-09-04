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
  and String result-API surfaces demand-first toward the audited head, one
  session-sized slice each (`docs/features.md` records what lands):
  - [ ] **Self-hosted `Variant`** — replace the intrinsic `Ty::Variant` (8
    MIR instructions, 13 checker protocol arms,
    `crates/mojito-pliron/src/lower/variants.rs`) with upstream's pure-Mojo
    `struct Variant[*Ts: AnyType]` whose conformances are TypeList-conditional
    (`Copyable where Ts.all_conforms_to[Copyable]()`, ...). Prerequisites, in
    order: (1) a storage primitive the VM can execute and pliron can lay out
    (a compiler-provided `_VariantStorage[*Ts]` with `isa[T]`,
    `unsafe_set_active[T]`, `unsafe_ptr[T]`, `unwrap[T]`; the register VM has
    no byte-addressed union); (2) `comptime T = Self.Ts[i]` type bindings
    inside `comptime for` bodies (Tuple only unrolls element indices today);
    (3) infer-only method type parameters resolved from a closure's return
    type in ordinary struct methods; (4) pack-conditional
    `__deinit__`/`__eq__`/`__hash__`/`write_to` bodies through the new
    storage, then retiring the intrinsic arms, the `variant.*` capability
    rows, and the `utils/__init__.mojo` name shim. Not session-sized; each
    prerequisite is its own slice.

  String residues (2026-09; batch 1 landed the search/count/replace/join/
  split/splitlines/strip families, `Boolable`, `__mul__`, String as `Writer`,
  and StringSpan `Equatable`/`Boolable`/`in`): `String == StringSpan` needs a
  second same-arity `String.__eq__` overload, which would make the String
  struct's trait-dispatch symbol (`__trait_dispatch.__eq__` retargeted to
  `String.__eq__`) ambiguous — spell `view == string`; a view-returning call
  on a temporary receiver (`String("x").strip()`) rejects with "reference
  binding to a non-place expression" (the receiver anchor only handles bound
  places) — bind the receiver; a `StringSpan` argument does not convert to a
  `String` parameter (upstream's parameters are `StringSpan`; Mojito's are
  `String`) — use `to_string()`; `split(sep, maxsplit=-1)` is one defaulted
  overload where upstream declares two (call sites are identical); bare `if
  s:` truthiness stays with the collection-truthiness item below. Batch 2
  (case/predicates/justification) residues: `upper`/`lower` cover a
  simple-case subset (ASCII, Latin-1, Latin Extended-A, Greek, Cyrillic,
  `ß` -> `SS`) where upstream ships the full Unicode simple + special casing
  tables (`_unicode.mojo`, generated lookups — port when a fixture needs
  another script); the batch-2 members live on `String` only, not
  `StringSpan` (call them on `view.to_string()`); `isspace` drops upstream's
  `single_character` parameter; `count_codepoints`/`count_graphemes` still
  `raise` on invalid UTF-8 (upstream's are non-raising over trusted buffers);
  `__radd__` (`"lit" + s` on a StringLiteral left operand already dispatches
  through the mixed-string path, so only user-struct reflected operators are
  missing: the checker has no `__r*__` fallback). Batch 3 (bytes, parsing,
  iterators) residues: `atof` is correctly rounded only while the
  significand (≤ 19 digits) and the power of ten (≤ 22) stay exact, and
  Mojito prints NaN as `NaN` where upstream prints `nan`; the `next(it)`
  builtin does not exist (iterate with `for` or call `__next__`);
  `Span(unsafe_ptr=, length=)` takes a `Pointer[T, MutUntrackedOrigin]`
  (a `Pointer[T, origin]` parameter cannot bind through a constructor type
  application: Pointer parameters coerce only on exact origins, and the
  constructor path resolves type applications without origin slots, so
  `Span[Byte, origin_of(self)](...)` rejects with a type-argument count);
  `Pointer`'s `[unsafe_offset=]` keyword subscript (upstream deprecates the
  positional form); `codepoint_slices_reversed`/`graphemes_reversed`/
  `__reversed__`, `bytes()`, `split_at_grapheme`, and `peek_next` on the
  iterators; and `count_codepoints`/`count_graphemes` (used by the iterators'
  `__len__`) still decode through an eager `to_string()` copy per step.

  Repr residues (2026-09; the repr slice landed `_unqualified_type_name[T]()`,
  `TypeNames`, upstream's scalar/String/Tuple/Slice repr texts): upstream's
  `FormatStruct(writer, "Name").params(...).fields(...)` builder needs
  variadic-pack methods on a struct (`def fields[*Ts: Writable](self, *args:
  *Ts)` — the elaborator reports "'Ts' is not a compile-time type" for a
  method-level pack, while the same shape on a free def specializes) plus a
  `ref` field to the writer; `write_repr_to` on `Optional` and other
  non-variadic generic structs needs either per-specialization checking of
  the body or runtime type bindings (the VM runs one erased body, and an
  `Optional[Int]` value carries no reified `T`, so
  `_unqualified_type_name[Self]` spells `Optional[T]`); `List`/`Dict`/`Set`
  repr falls back to the field-wise `Name(field=value)` text (upstream:
  `List[SIMD[DType.int, 1]]([Int(1), Int(2)])`); the bundled `Writable`
  trait declares only `write_to`, so a bounded `T: Writable` cannot call
  `write_repr_to` (spell `repr(x)`); natively `repr` lowers only for
  Strings and without escapes (scalars, tuples, and structs reject); a
  subscript view temporary passed as a call argument at its source's last
  use (`writer.write(s[byte=a:b])`) still frees the source first — bind the
  view to a local (the `$arg_loan_r` anchor covers call and method-call
  temporaries only); and upstream spells `_unqualified_type_name[Tuple[Int,
  Bool]]()` as `Tuple[<unprintable>, {}]`, so shared fixtures avoid tuple
  type names.

  Tuple residues (2026-09; the Tuple slice landed `Hashable`/`Sized` and
  upstream's `__contains__[T: Equatable]` bound): `Defaultable` needs
  `Ts[i]()` element default construction (the `Self.T()` blocker shared with
  Array), and the static `__len__()` overload cannot coexist with the
  instance one under arity-keyed dunder selection.

  Optional residues (2026-09; the Optional slice landed declared
  `Boolable`/`Defaultable`, conditional `Equatable`/`Hashable`/`Writable`,
  the `@implicit` value constructor, consuming `or_else`,
  `unsafe_value`/`unsafe_take`/`bounds`): `opt == None` needs same-arity
  dunder overload selection (`__eq__(rhs: Self)` beside `__eq__(rhs:
  NoneType)` — `struct_dunder_signature` and the VM's
  `call_resolved_dunder` key dunders by arity; record a `ResolveCallable`
  adjustment on the infix so `BinOp.resolved` names the exact overload),
  `__invert__` needs a `~` prefix operator, the raising `opt[]` subscript
  needs the empty-subscript form on nominal receivers plus
  `EmptyOptionalError`'s `TypeNames` text, and `copied`/`OptionalReg` need
  self-type-constrained methods / `TrivialRegisterPassable`.

  Variant residues (2026-09; the Variant slice landed the `init_with=`
  constructor, `unsafe_get`, static `is_type_supported`, the repr frame,
  and native hashing/equality of Variants): native `print`/`String(v)`/
  `repr(v)` of a Variant value (the VM forwards `write_to` to the payload;
  pliron's print and repr paths have no Variant arm), the payload repr text
  inside the frame now spells `Int(7)`/`'mojo'` like upstream), and
  `ref r = v.unsafe_get[T]()` as a place (the projection `v[T]` is the
  place form), and the projection tag-mismatch trap categories: the VM
  raises a `TypeError` (`Variant holds 'Int', not 'String'`) where native
  traps `UnhandledError`, so no error-differential fixture pins it.

  Slice residues (2026-09; the Slice slice landed `Slice.__eq__`, the
  `Slice(start, end, step)` writer text, the two-element
  `ContiguousSlice.indices`, native explicit construction, and the
  `Slice -> StridedSlice` widening): `Slice(...)` construction reads only
  `Int`/`None` argument expressions (`infer_slice_construction`; an
  `Optional[Int]`-typed variable needs the nominal Optional's slot read), and
  the explicit `.write_to(writer)` method spelling on a descriptor is not
  wired (print/`String(x)`/`Writer.write` are).

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
  - `(*, unsafe_uninit_length)` construction/resize (blocked on an
    uninit-element storage story for List, MaybeUninit-adjacent)
  - container `Hashable` conformances on the hasher protocol
    (`List`/`Optional`/`Array`/`Set`/`Dict.__hash__`; upstream
    discriminates `Optional`/`Variant` alternatives with a `UInt8` tag before
    delegating — `String`/`StringSpan`/`Tuple` already conform)
  - the upstream-exact `Hasher` member spellings: `_update_with_simd(mut
    self, value: SIMD[_, _])` with `SIMD.to_bits()` (Mojito narrows the leaf
    to one normalized `UInt64`, which both bundled hashers mix identically,
    so both backends' `UInt64` Variant discriminant lane is bit-identical to
    upstream's `UInt8` tag until this lands), the keyed `AHasher[key: U256]`
    (Mojito's is key-less; the seeded initializer remains), and the bytes
    `hash(bytes: ImmPointer[UInt8, _], n)` overload and `hash_seeded_bytes`
    (the pointer-backed `Span(unsafe_ptr=, length=)` constructor they need
    landed 2026-09 over an untracked pointer; `StringSpan.__hash__` feeds
    `as_bytes()` to `_update_with_bytes` without copying)
  - CTFE `hash`/`default_comp_time_hasher` for compile-time dictionaries
  - propagating a constructible type argument through an enclosing abstract
    binder on the raw seam (a generic body forwarding its own `H` into
    `hash[H](x)` without specialization): the reified argument spells the
    binder's name, and the VM's `ConstructTypeParam` falls back to the
    declaration default instead of the caller's binding (the compiled
    discovery path specializes the clone and is unaffected)

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
