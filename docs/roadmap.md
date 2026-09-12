# Mojito Roadmap

The single task tracker: an ordered checklist of **unfinished** work we
intend to do. Two kinds of entry leave this file. Completed work goes to the
supported surface in [`docs/features.md`](features.md), user-visible history
in [`CHANGELOG.md`](../CHANGELOG.md), and lasting design invariants in
[`docs/architecture.md`](architecture.md). Work we have decided *not* to do —
deferred options, capabilities kept on purpose, and limits that match
upstream — goes to [`docs/non-goals.md`](non-goals.md). North star:
self-hosting — prefer the smallest honest language change that unlocks a real
library pattern, with positive and negative tests.

Sections and their checkboxes are in implementation order; the first unchecked
box is the default next task. *(recurring)* and *(any order)* sections are
exempt from the ordering.

Sections 1 and 2 are ordered differently: every checkbox there, and every
bullet inside a checkbox that holds more than one independent item, names the
model that should take it, and they are sorted Opus before Fable. Each Opus
entry also says whether it can be started as-is or wants a plan first. Where a
strict dependency forces a different order, the entry says so.

Sections 1 to 3 are ordered by what the Pliron direction in
[`docs/pliron-future.md`](pliron-future.md) depends on: the existing native and
parity defects first, then the front-end groundwork a Pliron-centered
architecture would need.

## Ordered Work

### 1. Native Backend

Sorted Opus first (see **Entry Style**). The ABI-bump collector is last
whatever its model, because it batches every change that needs a new
`MJRT_ABI_VERSION`.

- [ ] **Front end: a bare literal cannot build a multi-lane SIMD field**

  Problem: `P(1)` for a struct whose field is `SIMD[DType.int32, 4]` is
  rejected with a field type mismatch.
  - Upstream accepts it through the implicit `SIMD(IntLiteral)`
    initializer.
  - Spell `P(SIMD[DType.int32, 4](1))` until then.
  - Model: Opus, plan first. The coercion rule itself is one site, but it
    fires at every field initialization, so the plan's job is to bound the
    overload-resolution and fixture fallout before the edit.

- [ ] **Native generic-holder temporaries with heap-owning implicitly
  copyable fields read freed memory**

  Problem: `GenericHolder[Box](Box(5))`, or Dict's
  `DictEntry[Optional[Int], V]` appended to a `List`, reads freed memory
  natively while the VM is correct.
  - This keeps `Dict[Optional[Int], _]` VM-only:
    `conformance/fixtures/optional_dict_keys.mojo` stays out of
    `assets/ok`.
  - Bisect the `var` field-move of the copy-lifecycle value inside the
    generic constructor against the owned-temp marking in
    `crates/mojito-pliron/src/lower/calls.rs`.
  - Model: Fable. The lever is a guess rather than a known site, and the
    bug sits where monomorphized constructors, copy lifecycle, and native
    temporary ownership meet.

- [ ] **Front end: no field writes through a `List` subscript**

  Problem: `xs[i].field = v` fails MIR verification (`dynamic element
  projection requires checked indexed storage`) for every element type.
  - Copy the element out, mutate it, and write it back (`xs[i] = e`), or
    mutate through a `mut` parameter, which works.
  - Model: Fable. A dynamic element projection that is written through
    crosses the checker, MIR places, the verifier, ownership and drop
    elaboration, and both backends.

- [ ] **Native runtime ABI bump: land every change that needs a new
  `MJRT_ABI_VERSION` together**

  Problem: each item below changes the native runtime ABI, so it needs an
  `MJRT_ABI_VERSION` bump and a normative `docs/native-abi.md` decision.
  They ship as one bump rather than one each.
  - Vector arguments and results never reach a register. A multi-lane
    SIMD value passes by pointer and returns through the sret
    out-pointer, so the caller's argument slot and the callee's result
    slot are addresses and never promote.
  - Local slots do promote, so that cost is confined to call boundaries:
    a `SIMD`-taking kernel reloads its parameter from memory.
  - The fix is to classify multi-lane SIMD as an LLVM vector in the
    function signature. Design record:
    `docs/notes/native-simd-pliron-assessment.md` §Recommended
    Representation Boundary.
  - The lane-index trap has no category of its own. An out-of-range lane
    index exits natively with the `unhandled error` category, while the
    VM raises `SIMD lane index … out of range`.
  - The fix is a dedicated trap category in `mojito-runtime`. Until then
    the parity harness cannot map the VM error, so the trap is pinned by
    `tests/pliron_opt_regression_test.rs` instead of an
    `assets/runtime_error` fixture.
  - Three runtime rows are dead code the bump should delete:
    `mjrt_fmt_i64`, `mjrt_fmt_u64` (integer display moved to the bundled
    `_int_digits`/`_uint_digits` Mojo bodies, 2026-09-12) and
    `mjrt_repr_string` (`repr` of a String moved to the compiled
    `String.write_repr_to`, same day). Nothing emits calls to them; the
    rows, the runtime implementations, and their `docs/native-abi.md`
    entries all go together.
  - A later task that needs an ABI bump joins this entry rather than
    getting its own.
  - Model: Fable. The signature classification changes the native calling
    convention itself, and the ABI version, `docs/native-abi.md`, the
    runtime, and the parity harness all move together.

### 2. Catch Up To Current Mojo *(recurring — reopens at every nightly re-pin)*

- When the pinned nightly moves: re-pin [`docs/mojo-nightly.md`](mojo-nightly.md),
  re-probe [`conformance/parity.tsv`](../conformance/parity.tsv), and burn
  down the divergences (`parity.tsv` notes, and the `mojito-only` /
  `mojo-only` rows of `conformance/cases.tsv`).
- Rule: Mojito matches or subsets Mojo. An extension is admitted only when
  it tracks an announced upstream direction (today: direct `ref` struct
  fields), is listed in [`docs/non-goals.md`](non-goals.md), keeps its
  fixtures under `assets/extensions/`, and is re-probed at every re-pin.
- The `a79fbdf59f2` pass (2026-08-26, Mojo `1.1.0.dev2026082605`) is
  complete (`docs/mojo-nightly.md`). The next re-pin recreates this
  section's checkbox.

The three checkboxes below, and the bullets inside the two standing ones,
are sorted Opus first (see **Entry Style**).

- [ ] **The rest of `assets/` has never been checked against the pinned Mojo**

  Problem: the ordinary `assets/` folders may hold only programs the pinned
  Mojo compiles, but the only sweep so far covered the files containing
  `comptime if`. It found eight rejected `assets/ok` fixtures, now respelled
  and listed in `conformance/cases.tsv`; the other ~800 fixtures are
  unswept (499 of them in the three `_ok` folders).
  - Run the pinned Mojo over every file in `assets/ok`, `assets/ownership_ok`,
    and `assets/origin_ok`, and respell or relocate what it rejects.
  - Fixtures in the error folders are their own question: the pinned Mojo
    rejects them too, but for its own reason, so a sweep there compares
    diagnostics rather than exit codes. Sweep the `_ok` folders first.
  - Each respelling that survives joins `conformance/cases.tsv` as a `run`
    row, the way the eight did, so nothing silently drifts back.
  - Model: Opus, plan first. The sweep itself is mechanical, but its size is
    unknown, and the plan's job is to bound how many fixtures need real
    respelling before the pass starts.

- [ ] **Mojito-specific shortcuts to move toward Mojo's shape** *(standing,
  any order)*

  Problem: parts of Mojito's stdlib lean on the Rust runtime where upstream
  is pure Mojo. Each is a candidate port, preferred over any new bridge
  (2026-09-07 direction).
  - The literal-filled `String.__init__(literal)` and
    `StringSpan.__init__(literal)` constructors, and the
    `String._as_string_literal()` struct-to-literal bridge behind
    `String.write_to`. Upstream: `StringLiteral` is a `StaticString` and
    `write_to` writes the bytes.
    - Model: Opus, plan first. The port only works once `StringLiteral` and
      `String` agree on a runtime representation, which is also what the
      erased-path residue in section 4 needs, so plan the representation
      before touching the constructors.
  - Float formatting in Rust: the float arm of the VM's `Display for Value`
    and the native `mjrt_fmt_f64`. Upstream formats `Float64` in Mojo
    (Dragonbox in `format_float`).
    - Model: Opus, plan first. Transliterate a permissively licensed Rust
      Dragonbox (MIT or Apache-2.0 — third-party crates are allowed, see
      `AGENTS.md`) into Mojo rather than deriving the algorithm; the plan
      picks the source and pins the shortest-round-trip cases. Only a
      from-scratch derivation would want Fable.
  - The VM's `Display for Value` still renders `Int`/`UInt` in Rust. The
    2026-09-12 integer-formatting port moved every program-visible text
    path — `print`, `String(...)`, `repr`, `Writer.write`, format
    templates — to the bundled `_int_digits`/`_uint_digits` bodies, but a
    `fmt::Display` impl has no VM to call them with, so three callers keep
    the Rust arms: diagnostics, the CLI binding dump, and `SIMD` lanes.
    - Model: Opus, as-is. Each caller needs a VM-aware renderer, or a
      reason to stay Rust.

  Four runtime services are deliberately not on that list; they are in
  [`docs/non-goals.md`](non-goals.md).

- [ ] **Behavioral divergences from the pinned Mojo — burn to zero**
  *(standing)*

  Every new divergence lands here with a probe or a `cases.tsv`
  `mojito-only` / `output-diff` row, and leaves when its probe promotes to
  an `assets/ok` fixture.

  Open today:
  - `unsafe-origin-cast-mutability`: `unsafe_origin_cast` accepts a target
    whose mutability differs from the pointer's (an `ImmOrigin(o)` or
    `ImmUntrackedOrigin` target on a mutable pointer), while upstream's
    `target_origin: Origin[mut=Self.mut]` should reject it. Mojito rejects
    only the upgrade direction. Pinned by
    `conformance/probes/unsafe_origin_cast_mutability.mojo`.
    - Model: Opus, as-is. One missing half of a comparison Mojito already
      makes.
  - `simd-subscript-indexer`: Mojito normalizes any `Indexer` through
    `__mlir_index__` at every subscript, so a `SIMD` lane accepts one,
    while upstream's `SIMD.__getitem__` takes a plain `Int` and rejects it.
    A user type's own `__getitem__` does take an `Indexer` upstream
    (`assets/ok/indexer_normalization.mojo`), so only the builtin SIMD
    subscript diverges.
    - Model: Opus, as-is. Exempt the builtin SIMD subscript from the
      normalization the rest of the subscripts keep.
  - `live-pointer-ref-argument`: a `ref` argument naming a place is
    rejected while a `Pointer(to=place)` to it is still live (`access to
    'x' conflicts with live reference 'p'`). Upstream accepts it, because
    `Pointer` is not an exclusive borrow. Pinned by
    `conformance/probes/live_pointer_ref_argument.mojo`.
    - Model: Opus, as-is. A `Pointer` loan is already shared
      (`MirLoan::shared`); the conflict rule has to read that.
  - `mut-pointer-parameter-reassignment`: assigning to a `mut` parameter of
    `Pointer` type inside the callee (`mut p: Pointer[Int, o]`, then
    `p = p`) is rejected by MIR verification (`WriteRef value type
    Pointer[Int, origin#0] is incompatible with referent Int`), while
    upstream accepts it. The store lowers as a `WriteRef` through the
    parameter's slot handle, which is typed as the pointer itself, so the
    verifier expects a pointee-typed value. Copying the parameter works.
    Pinned by `conformance/probes/mut_pointer_parameter_reassign.mojo`.
    - Model: Opus, as-is. The mistyped handle is diagnosed and sits in MIR
      lowering.
  - `comptime-if-module-level`: a `comptime if` at module level is accepted,
    while upstream requires one inside a function (`'comptime if' must be
    contained in a function`). No fixture spells it any more — the two that
    did now fold their assertion into a second `comptime` alias — so only
    the rejection is left to write, with a `parse_error` fixture to pin it.
    - Model: Opus, as-is. A parse or check-time rejection; the fixture
      respellings are done.
  - `is-same-type-builtin`: `is_same_type[T, U]()` is a Mojito-only
    compile-time predicate; upstream has no such declaration and compares
    type values with `==`. The stdlib and `assets/ok` now spell `==`; what
    is left is the builtin itself, still reachable from user code and still
    spelled by `assets/type_error/type_predicate_runtime_if.mojo` and eight
    `tests/comptime_test.rs` cases.
    - Model: Opus, as-is. Delete the intercept in
      `crates/mojito-comptime/src/comptime/{eval,ctfe,elab}.rs` and respell
      its remaining callers.
  - `pack-element-type-narrowing`: inside a folded `comptime if Self.Ts[i]
    == T` branch Mojito treats the pack element `self.storage[i]` as a `T`,
    so a `ref[origin_of(self)] T` accessor returns it and an `==` against a
    `T` argument type-checks. Upstream keeps the element at its dependent
    pack type and demands `rebind[T](...)`, which Mojito does not
    implement; it also rejects a reference into `self.storage` returned
    under `origin_of(self)`. `stdlib/std/collections/tuple.mojo`'s
    `__contains__` relies on the same narrowing. Pinned by
    `conformance/fixtures/pack_element_type_narrowing.mojo`
    (`pack-element-type-narrowing`).
    - Model: Opus, plan first. Closing it means implementing `rebind` and
      then requiring it, so the plan decides whether `rebind` lands first
      or the two land together.
  - `trivially-movable-stdlib-types`: `IsTriviallyMovable[String]`,
    `IsTriviallyMovable[List[Int]]`, and the `MaybeUninit` conformances
    that follow from them are `False` on Mojito and `True` upstream,
    because six bundled stdlib types (`String`, `List`, `Array`, `Dict`,
    `Set`, `Optional`) declare an explicit `__init__(out self, *, deinit
    move: Self)` where upstream relies on the implicit bitwise move. The
    predicate itself agrees on hand-written structs.
    - Model: Opus, plan first. Deleting the six move constructors is the
      fix, but it hands every heap-owning move to the compiler-generated
      path on both backends, so the plan checks that path first.
  - `ref-binding-register-value`: a `ref` binding to a register-passable
    value is rejected upstream (`value of type 'Int32' doesn't have a
    memory origin in 'ref' binding`) but accepted by Mojito. It covers
    `ref y = f()` for an `Int`-returning `f`, `ref y = a + 1.0`, and a SIMD
    lane read `ref lane = v[i]`. The lane binding also writes through to
    the vector, so `lane += 2` changes `v`. A memory-backed temporary
    (`ref x = make_list()`) is accepted by both. The lever is the
    `StmtKind::RefDecl` arm in
    `crates/mojito-checker/src/checker/statements.rs`, whose
    `materialized_reference_actual` fallback materializes any value.
    Pinned by `conformance/probes/ref_binding_register_value.mojo`.
    - Model: Opus, plan first. The lever is named, but the rejection reaches
      wide fixture fallout, so it wants its own pass with the fallout
      enumerated first.
  - `bare-pack-parameter-in-method`: a method may name its struct's pack
    parameter bare (`Ts.length`, `Ts[i]`), while upstream demands `Self.Ts`
    there (`unqualified access to struct parameter 'Ts'; use 'Self.Ts'
    instead`) and reserves the bare name for the struct's own conformance
    clauses, where `Self` is unavailable.
    `assets/ok/variadic_pack_upstream_spellings.mojo` now spells both the
    upstream way.
    - Model: Fable. A leniency to withdraw, whose fallout is every stdlib
      and fixture method that names a pack.
  - `partial-field-move-parent-used`: moving one field out of a struct
    (`p.a^`) while the parent is used afterwards (`p.b`) is rejected
    upstream (`value 'p.a' cannot be consumed, because 'p' is used later`)
    but accepted and tracked field-wise by Mojito. The Mojito behavior is
    pinned by
    `tests/drops_test.rs::partially_moved_field_is_dropped_once_at_its_new_owner`.
    - Model: Fable. Ownership goes from field-wise to whole-parent, which
      changes what the analysis tracks rather than what it spells.
  - `symbolic-origin-pointer-write`: a write through a pointer field whose
    origin binder has symbolic mutability (`Origin[mut=m]`, or a bare
    `Origin`) is accepted however the binder was bound, while upstream
    judges the binder per instantiation and rejects the write from an
    immutable place (`expression must be mutable in assignment`). Origin
    arguments are erased from checked identity, so no per-instance binding
    reaches `check_pointer_write`; carrying it there is the fix. An
    `ImmOrigin(o)` origin argument (`Cell[ImmOrigin(o)]`) erases the same
    way, so a write through that view's pointer field is accepted too.
    - Model: Fable. Origin arguments must survive checked identity, which
      erases them today.
  - `assign-view-over-source`: assigning a call straight back to a local
    that one of its view arguments borrows is judged by origin upstream but
    by temporary lifetime in Mojito. Upstream rejects a view at
    `origin_of(s)._get_owned_interior["bytes"]` (`s[byte=..]`,
    `s.rstrip()`) with `aliasing values passed immutably to 'v' argument
    and constructed as a result in 'takes' call`. It does so even through a
    named local (`var r = s.rstrip()` then `s = String(r)`), which Mojito
    accepts. It accepts a plain-origin view (`s = takes(StringSpan(s))`),
    which Mojito rejects because the temporary's loan is live at the store.
    The fix is to declare upstream's owned-interior origins on the `String`
    view methods and check argument origins against the assignment
    destination in the checker. Pinned by the `assign-*-view-over-source`
    rows of `conformance/cases.tsv`.
    - Model: Fable. It introduces owned-interior origins to the stdlib's
      view methods and a new checker rule that reads them.
  - `comptime-if-dropped-branch`: a type error inside the untaken branch of a
    `comptime if` is never reported, because elaboration drops the branch
    before the checker runs. Upstream type-checks every branch symbolically
    and rejects the program (`var x: Int = "hello"` in the untaken branch, and
    `x.nonexistent()` on a `T: Copyable` parameter).
    - Model: Fable, and strictly dependent: section 3's first task is the
      fix, so this row closes there and not here.

  Three divergences are retained on purpose and re-probed rather than fixed;
  they are listed in [`docs/non-goals.md`](non-goals.md).

### 3. Front-End Groundwork For A Pliron-Centered Architecture

The assessment is [`docs/pliron-future.md`](pliron-future.md): Pliron cannot
give Mojito Mojo's shape on its own, because Mojo type-checks parametric code
before instantiating it while Mojito elaborates first and checks the clones.
These tasks fix that order. Each one pays off by itself, the first closes a
conformance divergence, and together they are what a later Pliron pivot would
need. Start them only once sections 1 and 2 are clear.

- [ ] **A type error in an untaken `comptime if` branch is never reported**

  Problem: elaboration drops the untaken branch before the checker runs, so
  Mojito executes programs the pinned Mojo rejects.
  - Upstream rejects `var x: Int = "hello"` in the untaken branch with `cannot
    implicitly convert 'StringLiteral["hello"]' value to 'Int'`.
  - Upstream also rejects `x.nonexistent()` on a `T: Copyable` parameter in an
    untaken branch. Mojito runs both programs.
  - Upstream type-checks a generic body symbolically before instantiating it
    ("Type check + Generate IR before instantiating", LLVM Dev Meeting 2025).
  - The fix is to check every branch with its parameters left symbolic and
    select afterwards. The lever is `comptime::elaborate` running ahead of
    `checker::check_program`.
  - `docs/architecture.md` §Stage 2 documents the dropped branch as intended
    behavior, so it changes with this task.
  - Land the two probes under `conformance/probes/` first.
  - Known fallout, from the `comptime if` sweep recorded in
    [`docs/pliron-future.md`](pliron-future.md): two fixtures put a deliberate
    type error in the untaken branch as a compile-time assertion
    (`assets/ok/generic_ctfe_value_param.mojo`,
    `assets/ok/generic_ctfe_associated_value.mojo`).
  - Two more sites rely on the guard narrowing a method's `T` to the element
    type: `assets/ok/variadic_method_type_params.mojo` (`get`,
    `count_matching`) and `stdlib/std/collections/tuple.mojo`
    (`__contains__`).
  - Upstream spells that shape `rebind[Ts[i]](value)`, which Mojito does not
    implement (`Undefined variable 'rebind'`), so this task needs `rebind` or
    an equivalent before those two sites can be respelled.

- [ ] **Parameter expressions have no symbolic form**

  Problem: `CtValue` carries concrete values plus an opaque symbolic `Param`,
  so two parameter expressions can only be compared by making them concrete.
  - `SIMD[dt, n + 1]` and `SIMD[dt, 1 + n]` cannot be judged equal today.
  - Symbolic branch checking needs this, and so does any parametric IR.
  - Upstream stores parameter expressions as uniqued typed attributes and
    decides equality by canonicalization rather than evaluation.
  - Shape the representation like a Pliron attribute, uniqued and
    canonicalized, so a later dialect move is a re-homing and not a redesign.

- [ ] **The Pliron pivot has no falsifiable proof yet**

  Problem: [`docs/pliron-backend-pivot-plan.md`](pliron-backend-pivot-plan.md)
  stages a migration to a required Pliron IR framework, but its Stage A1 slice
  has never been built, so the decision rests on paper.
  - Build the A1 vertical slice from that plan's §Smallest falsifiable proof.
  - Measure construction time, verification time, peak memory, and text size
    against the budget the plan names.
  - Decide from the measurements: continue to A2, or record the rejection in
    [`docs/non-goals.md`](non-goals.md).
  - This is the decision point for MIR-as-a-dialect, not a commitment to it.

### 4. Grow The CPU Standard Library *(demand-first)*

- [ ] **Collection API parity**

  Goal: grow the tuple, slice, optional/variant, and String surfaces toward
  the audited head (`docs/features.md` records what lands). The tasks below
  are in impact order: soundness of the executable oracle first, then
  everyday spellings that reject today, then parity details. No task
  depends on a later one. Every bullet is a conscious, recorded limit, a
  task closes when its bullets are done, and a residue found inside a task
  moves to the task that owns its fix.

  1. **Compile-time evaluation residues** — what VM CTFE can bind and
     resolve.
     - A VM-evaluated compile-time expression whose result is
       pointer-backed (`comptime C = M.copy()`, a bare `Optional`, a
       `String`) cannot cross back (`cannot cross back from VM CTFE`).
       Upstream binds it. The freezable results are scalars, Bool, String,
       tuples, fieldwise structs, and displays.
     - Compile-time Dict/Set key identity is structural `CtValue`
       equality. That is exact for every prelude key type, and diverges for
       a user-struct key with a non-fieldwise `__eq__`.
     - The typing probe checks the CTFE subprogram once more per VM-bound
       expression.
     - A non-parameterized struct alias mentioning a value parameter
       (`comptime Alias = S[Self.T, Self.length]`) resolved through an
       instance (`S[Int, 3].Alias`) keeps `length` symbolic (`expected
       S[Int, length], found S[Int, 3]`). `associated_type_from_base`
       substitutes type parameters only; parameterized aliases substitute
       both.

  2. **Monomorphization coverage and cost** — where the erased path still
     stands in for an instance clone, and what minting costs.
     - An instantiation whose argument mentions `StringLiteral` (`{"a": 1}`
       is `Dict[StringLiteral, Int]`) keeps the erased path: its values keep
       the literal runtime representation while an un-annotated binding
       materializes `String`. Lifting it needs one runtime representation
       for `StringLiteral` and `String`, or typing the display as
       `Dict[String, Int]` as Mojo does. `StringDict.__getitem__` stays a
       `Copyable`-guarded copy for the same reason: its
       `StringLiteral`-keyed entry list keeps the erased path, where a
       reference result cannot spell its interior origin.
     - A call inside an unstamped bundled body on a bundled struct's
       generic method keeps the erased path. Requests are admitted from
       user code, clone bodies, variadic specs, instances, and user
       structs.
     - An instance clone whose walk cannot resolve an application (a
       variadic template over a nested public `Tuple` argument) is dropped
       to the erased path rather than failing the program.
     - Value-parameterized structs get no instance clones, so an erased
       body's `_unqualified_type_name[Self.T]()` spells `T` (`repr([1, 2])`
       prints `Array[T, 2]([Int(1), Int(2)])` where upstream prints the
       element type). A `Self.n` bracket argument inside such a body
       (`Counter[Self.length](i)`) is VM-only: native monomorphization
       needs a compile-time-constant value argument (`unsupported value
       parameter 'length' is not compile-time constant`).
     - Clones are minted per whole instance with no reachability pruning
       and re-checked each discovery round. `benchmarks/compile/stdlib_heavy`
       is about 2.2x its pre-clone baseline in release
       (`docs/performance.md`), and the repr methods added 2026-09-05 cost
       about 5% in debug across the compile benchmarks. Lever:
       reachability-pruned minting.

  3. **Variadic packs and tuples** — what the pack machinery types
     syntactically or refuses.
     - Type-pack calls inside a nested `def` and whole-pack-forwarded calls
       keep the syntactic element-typing path (`a heterogeneous pack
       specialization needs an expression whose type is statically evident
       before checking` for `take(b, 1)` over a local `b`). Top-level calls
       consult the checker's instantiation. Lifting it needs four pieces:
       nested templates live only in `NestedMono` and are deleted by
       `replace_templates` before the checker sees them, the checker would
       have to accept a nested variadic shell abstractly,
       `def_specialization_requests` is top-level only, and
       `Mono.def_call_targets` must reach `NestedMono::scan_expression`
       with a `def_request_target` fallback.
     - A user variadic struct application as a pack element
       (`Tuple[TypeNames[Int]]`) keeps the fixed-arity diagnostic. Its
       erased shell has no sound nominal form; the public `Tuple` is the
       one compiler-known template whose `*Ts` absorbs every argument.
     - A public Tuple as an explicit type argument does not conform to
       `Deinitable` (`make[T: Defaultable & Deinitable]()` over
       `Tuple[Int, Bool]` reports the bound failure), while the inferred
       shape runs.
     - A standalone nullary vector construction `SIMD[d, w]()` rejects
       (`SIMD construction expects w element(s) or 1 to splat, got 0`).
       Only a Tuple element defaults to zero lanes.

  4. **Everyday spellings that still reject** — checker context and stdlib
     API shapes.
     - A list display as an argument to an explicitly applied constructor
       at runtime (`Dict[String, Int](["a"], [1], None)`) takes no context
       from the parameterized `List[Self.K]` parameter and materializes as
       `Array`, so no overload matches. Spell the lists
       (`List[String]("a", __list_literal__=None)`), as a compile-time
       value's materialization does.
     - A struct's own value parameter as a SIMD width in a signature
       (`def zeros(self) -> SIMD[DType.int64, Self.length]`) reports `SIMD
       width must be a positive power of two, got a type`. The width slot
       needs a concrete comptime `Int`, where upstream binds the parameter.
     - String literals have no methods (`"abc".byte_length()` rejects). A
       literal-typed value converts through `String(...)` first.
     - An annotated `Optional[T]` local initialized from a bare struct
       construction (`var o: Optional[P] = P(4)`) fails MIR verification
       with an untyped register for the constructor call. Spell the wrapper
       (`Optional[P](P(4))`, `Optional(P(4))`); a literal payload
       (`var n: Optional[Int] = 6`) converts.
     - `if`/`while` accept a width-1 bool lane through the `Bool(x)`
       truthiness conversion, but `and`/`or` still demand `Bool` operands
       (`a == b or c < d` over `UInt64`s rejects). Nest the tests.
     - A string literal does not bind a trait-bounded type parameter the
       nominal `String` satisfies (`isdir("/tmp")` reports `'StringLiteral'
       ... does not conform to trait 'PathLike'`). Spell `String("/tmp")`.
     - Struct-level `comptime NAME = Self(n)` constants (upstream's
       `ErrNo.ENOENT`) do not fold. The associated-constant evaluator
       handles prefix, infix, tuple, and list expressions only.
     - `@fieldwise_init` beside a hand-written `__init__` rejects. Spell
       every constructor by hand.
     - A bound-generic def whose template body calls itself with a concrete
       argument (`makedirs(head, exist_ok=...)` inside `makedirs[PathLike]`)
       mints its own specialization while checking and reports it
       undefined. Recurse through a non-generic helper.
     - Nullary construction of a sized-scalar array (`Array[Int8, 1024]()`)
       reports the scalar's `Defaultable` construction unsupported. Spell
       `Array[Int8, 1024](fill=0)`.
     - `Pointer.unsafe_bitcast[U]()` is not typed: an origin-cast-style
       forwarding leaves the MIR register at the old element type.
     - `CStringSlice` views `Byte` elements instead of upstream's `c_char`.
     - No `std.builtin.rebind.downcast`: `Dict`/`Set` guard `keys`,
       `values`, `items`, and `__iter__` with `where conforms_to(K,
       Copyable)` instead of laundering the parameter. `Dict.keys` needs
       copyable values as well as keys, since the key view wraps the entry
       view.

  5. **Naming and Unicode details** — output text only.
     - `_unqualified_type_name` spells nested structs unqualified where
       upstream keeps a non-prelude struct's module path
       (`Optional[std.collections.dict.Dict[...]]`, `List[up.Flag[True]]`).
       `List`, `Optional`, `String`, and `SIMD` stay bare.
     - Grapheme segmentation is the documented UAX #29 essentials subset:
       hand-maintained Control/Extend/SpacingMark ranges, no
       Extended_Pictographic or Prepend data. Reverse iteration re-scans
       forward from the nearest CR/LF/Control boundary.

- [ ] **Filesystem and I/O residues**

  Behind the landed files, streams, paths, and tempfile stage
  (`docs/features.md`).

  1. The remaining tail, each a small port:
     - public `stat` / `lstat` / `stat_result`, `realpath`, `symlink` /
       `link` / `chdir`, and `isatty` (`FileDescriptor.isatty` / `fchdir`);
     - `OptionalPointer` (null tests spell `Int(ptr) == 0`);
     - `ErrNo`'s named constants;
     - `~user` expansion (`getpwnam`);
     - the VM's per-process environment overlay (native `setenv` writes the
       real environment);
     - `NamedTemporaryFile`;
     - `FileHandle.read` into a typed `Span[Scalar[dtype], origin]`;
     - `Path.stat` / `lstat` / `_dir_of_current_file`;
     - `KeyElement`;
     - `_get_random_name` over `std.random` (reads `/dev/urandom` today);
     - a generic `__exit__[E]`.
  2. Traps the stage worked around, each its own fix:
     - An overloaded constructor spelled with the `StringSlice` or `Byte`
       alias mangles a different key than the `StringSpan` / `UInt8` the
       call selects. The ports spell the canonical names.
     - A temporary view as a method argument
       (`s.take(String("x").as_bytes())`) is a VM "reference receiver must
       be a place" rejection, and as an augmented-assignment operand
       (`p /= StringSpan(s)`) a VM use-after-free.
     - `Span[mut=True, T, _]` fails origin inference. Spell an
       `[origin: Origin[mut=True]]` binder.
     - The two VM destruction-order divergences the stage found live in the
       behavioral-divergences task of section 2.
     - `range` is invisible in `std.string`.
     - A module loaded while the prelude bootstraps (`std.io` and the whole
       `std.os` graph now) must import `String` explicitly. That graph
       costs Hello World about a second of debug compile time
       (`docs/performance.md`).

- [ ] **Time, random, and testing slices**

  Goal: deterministic testable cores, with host-dependent behavior behind
  runtime services.

### 5. Packaging, Artifacts, And Developer Tooling *(any order unless noted)*

- [ ] **Compile-time performance**

  Problem: Hello World is 0.8 s release / 4.6 s debug
  (`docs/performance.md`; it was 24 s / 52 s before the shared
  `Arc<CheckedTables>`). Next, in order:
  1. Avoid the redundant checker passes. Hello World re-elaborates and
     re-checks once because the request scan always finds the prelude's own
     Tuple/def requests, and every check runs two transfer rounds.
  2. `checked_var_types` scans the whole expression table per variable, and
     `explicit_destroy` re-derives deinitability per struct per pass.
  3. Only then cache the elaborated/checked stdlib across processes.

- [ ] **Feature and target options**

  Goal: checked CLI/build configuration recorded in artifacts and
  diagnostics.

- [ ] **Compiled package artifacts**

  Goal: a versioned `.mojoc` representation (modules stay non-first-class).
  Per-directory resolution order:
  1. source package
  2. `.mojoc`
  3. source module
  4. legacy `.mojopkg`

- [ ] **Debugging metadata and inspection**

  Goal: stack/source diagnostics, MIR inspection, and debugger-oriented
  value rendering.

- [ ] **Testing tools**

  Goal: Mojito-native assertions, expected-error tests, and
  differential-harness integration.

- [ ] **The corpus sweeps no longer run in the overnight gate**

  Problem: the two generated pliron manifests and their coverage ratchets
  now only move when someone runs `scripts/check-pliron-heavy` by hand, so a
  regression in them can sit unnoticed for days.
  - They moved out of the gate on 2026-09-12, after the OOM killer took the
    parity harness: the sweeps in `tests/heavy/` peak at several gigabytes
    each and nextest ran them beside each other.
  - `support::compile_jobs` now sizes each sweep's fan-out against
    `MemAvailable` rather than the core count, so one sweep alone is safe.
  - What is missing is scheduling, not safety: a way to run the heavy lane
    unattended when the machine is otherwise idle, and to surface its result
    where the morning triage already looks.

- [ ] **Distribution reproducibility gate** *(last)*

  Goal: the release check rebuilds, tests, documents, and reproduces
  conformance from the crates.io archive alone.

### 6. Code Organization Follow-Ups *(any order — behavior-preserving)*

The 2026-09 module split (`docs/symbol-map.md`) removed every file over
3,000 lines. What remains needs semantic extraction, not line moves.

- [ ] **Split `expr_unconverted`**

  `mir/lower_expr/expr.rs` (about 2,090 lines) is one match over
  `ExprKind`.
  - Extract arm groups into `Flatten` methods.

- [ ] **Split `infer_method_call`**

  `checker/method_calls/mc_infer.rs` (about 1,530 lines) is one method.
  - Extract receiver-family branches beside `selection`, `statics`, and
    `builtin_types`.

- [ ] **Split `verify_instruction`**

  `mir/verify/instr.rs` (about 1,320 lines) is one match over `MirInstr`.
  - Extract per-family check helpers.

- [ ] **Shrink the 2 kloc band**

  Split these further only along a cohesive seam, while touching them:
  - `checker/traits.rs` (2,629), `mir/lower_stmt.rs` (2,595),
    `checker/inference.rs` (2,520), `checker/statements.rs` (2,471),
    `ast.rs` (2,464), `mir.rs` (2,426), `checker/type_resolution.rs`
    (2,395), `runtime.rs` (2,235), `checker.rs` (2,232),
    `comptime/rewrite.rs` (2,179), `checker/declarations.rs` (2,118),
    `mir/text/write.rs` (2,116), `backend/vm/exec.rs` (2,085).

## Task Lifecycle Policy

`roadmap.md` is the only task list: no parallel todo file, no retained
completed tasks.

- Unfinished work is an unchecked, outcome-oriented task in **Ordered
  Work**; design detail lives in plans or `docs/notes/`.
- A task we decide not to do is not left unchecked here. Move it to
  `docs/non-goals.md` with the reason and the condition that would reopen
  it.
- A task is complete only when implementation, focused positive and negative
  coverage, documentation, and `scripts/check` agree. In the same change,
  delete it here and record the outcome in `docs/features.md`,
  `CHANGELOG.md`, and (for design invariants) `docs/architecture.md`.
- Rewrite partially completed tasks so only the remaining outcome is stated;
  prefer one checkbox per independently demonstrable outcome.

## Entry Style

Every entry is written for a human reader who has not seen the code.

- One problem per checkbox. Its first sentence states the issue to fix. An
  item we are not going to fix does not belong here at all — it belongs in
  [`docs/non-goals.md`](non-goals.md).
- Details follow as short bullet points: the symptom, the workaround, the
  cause or the lever, and the file or test that pins it.
- No run-on sentences and no semicolon chains. If a thought needs a
  semicolon, it is two bullets.
- Code spellings appear only where the reader must go look
  (a file, a test, a diagnostic text, an example line).
- A residue list from a finished task becomes several checkboxes, not one
  paragraph.
- Exception: every change that needs an `MJRT_ABI_VERSION` bump shares
  one checkbox, so the native runtime ABI is bumped once for all of them.
- Sections 1 and 2 carry a **Model:** bullet on every checkbox, and on every
  bullet inside a standing checkbox that holds more than one independent
  item. It is a quick complexity and blast-radius estimate. Fable is for work
  that changes a contract, spans phases, or has no named lever; Opus is for
  work whose site and rule are both known.
- An Opus entry adds "as-is" or "plan first". "Plan first" means the lever is
  known but the fallout is not enumerated yet.
- Those two sections are sorted by that estimate, Opus before Fable, at both
  the checkbox and the bullet level. A strict dependency that forces another
  order is stated in the entry that carries it.

## Working Rule

1. Start with a self-hosted library or small user-facing acceptance case.
2. Record the current failure with a focused test.
3. Implement the smallest compiler change that makes the program honest.
4. Add positive and negative coverage at the owning compiler phase.
5. Run `scripts/check` before marking the task complete.
