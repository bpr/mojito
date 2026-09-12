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

Sections 1 to 3 are ordered by what the Pliron direction in
[`docs/pliron-future.md`](pliron-future.md) depends on: the existing native and
parity defects first, then the front-end groundwork a Pliron-centered
architecture would need.

## Ordered Work

### 1. Native Backend

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
  - A later task that needs an ABI bump joins this entry rather than
    getting its own.
  - Model: Fable. The signature classification changes the native calling
    convention itself, and the ABI version, `docs/native-abi.md`, the
    runtime, and the parity harness all move together.

- [ ] **Native SIMD: float-to-int casts convert one lane at a time**

  Problem: a vector float-to-int cast extracts each lane, saturates it
  through `llvm.fptosi.sat.i128.f64`, and re-inserts it.
  - The vector form (`llvm.fptosi.sat.v{N}i128.v{N}f64`) is legal IR, but
    its x86-64 legalization is unverified, so it was not adopted.
  - Probe it with `llc` before switching.
  - Model: Opus. One lowering site behind an `llc` probe, no contract
    change.

- [ ] **Front end: a bare literal cannot build a multi-lane SIMD field**

  Problem: `P(1)` for a struct whose field is `SIMD[DType.int32, 4]` is
  rejected with a field type mismatch.
  - Upstream accepts it through the implicit `SIMD(IntLiteral)`
    initializer.
  - Spell `P(SIMD[DType.int32, 4](1))` until then.
  - Model: Opus. A coercion rule at field initialization, with overload
    resolution the only thing downstream to watch.

- [ ] **Front end: no field writes through a `List` subscript**

  Problem: `xs[i].field = v` fails MIR verification (`dynamic element
  projection requires checked indexed storage`) for every element type.
  - Copy the element out, mutate it, and write it back (`xs[i] = e`), or
    mutate through a `mut` parameter, which works.
  - Model: Fable. A dynamic element projection that is written through
    crosses the checker, MIR places, the verifier, ownership and drop
    elaboration, and both backends.

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

- [ ] **Native: a view method on a temporary owned receiver reads freed
  memory**

  Problem: `for g in String("abc").codepoints()` and
  `len(String("abc").__reversed__())` trap natively with `use after
  Pointer deallocation`, while the VM and upstream run them.
  - The temporary receiver must live to the end of the statement, because
    the returned view borrows it.
  - A named receiver and direct iteration (`for g in String("abc")`) run
    natively, and so does a temporary `StringSpan(s)` receiver.
  - Start at the owned-temp release in
    `crates/mojito-pliron/src/lower/calls.rs`, which frees the receiver
    after its last use as an operand.
  - Pinned by `conformance/probes/native_temporary_receiver_view.mojo`.
  - Model: Opus. The rule is stated and the site is named: hold an owned
    temporary to the end of its statement in one pliron crate.

- [ ] **Native converting-constructor defaults are rejected**

  Problem: a `CheckedConst::Construct` default such as
  `arg: Optional[T] = None` is rejected at the native default-fill sites
  (`checked_const_value` errors on `Construct`).
  - Emit the constructor call at default-fill through `lower_call`.
  - The `NoneType` argument is `LowerTy::ZeroSized`.
  - Model: Opus. Two named sites in one crate, no semantic decision.

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

- [ ] **Behavioral divergences from the pinned Mojo — burn to zero**
  *(standing)*

  Every new divergence lands here with a probe or a `cases.tsv`
  `mojito-only` / `output-diff` row, and leaves when its probe promotes to
  an `assets/ok` fixture.

  Open today:
  - `partial-field-move-parent-used`: moving one field out of a struct
    (`p.a^`) while the parent is used afterwards (`p.b`) is rejected
    upstream (`value 'p.a' cannot be consumed, because 'p' is used later`)
    but accepted and tracked field-wise by Mojito. The Mojito behavior is
    pinned by
    `tests/drops_test.rs::partially_moved_field_is_dropped_once_at_its_new_owner`.
  - `symbolic-origin-pointer-write`: a write through a pointer field whose
    origin binder has symbolic mutability (`Origin[mut=m]`, or a bare
    `Origin`) is accepted however the binder was bound, while upstream
    judges the binder per instantiation and rejects the write from an
    immutable place (`expression must be mutable in assignment`). Origin
    arguments are erased from checked identity, so no per-instance binding
    reaches `check_pointer_write`; carrying it there is the fix. An
    `ImmOrigin(o)` origin argument (`Cell[ImmOrigin(o)]`) erases the same
    way, so a write through that view's pointer field is accepted too.
  - `unsafe-origin-cast-mutability`: `unsafe_origin_cast` accepts a target
    whose mutability differs from the pointer's (an `ImmOrigin(o)` or
    `ImmUntrackedOrigin` target on a mutable pointer), while upstream's
    `target_origin: Origin[mut=Self.mut]` should reject it. Mojito rejects
    only the upgrade direction. Pinned by
    `conformance/probes/unsafe_origin_cast_mutability.mojo`.
  - `simd-subscript-indexer`: Mojito normalizes any `Indexer` through
    `__mlir_index__` at every subscript, so a `SIMD` lane accepts one,
    while upstream's `SIMD.__getitem__` takes a plain `Int` and rejects it.
    A user type's own `__getitem__` does take an `Indexer` upstream
    (`assets/ok/indexer_normalization.mojo`), so only the builtin SIMD
    subscript diverges.
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
  - `live-pointer-ref-argument`: a `ref` argument naming a place is
    rejected while a `Pointer(to=place)` to it is still live (`access to
    'x' conflicts with live reference 'p'`). Upstream accepts it, because
    `Pointer` is not an exclusive borrow. Pinned by
    `conformance/probes/live_pointer_ref_argument.mojo`.
  - `mut-pointer-parameter-reassignment`: assigning to a `mut` parameter of
    `Pointer` type inside the callee (`mut p: Pointer[Int, o]`, then
    `p = p`) is rejected by MIR verification (`WriteRef value type
    Pointer[Int, origin#0] is incompatible with referent Int`), while
    upstream accepts it. The store lowers as a `WriteRef` through the
    parameter's slot handle, which is typed as the pointer itself, so the
    verifier expects a pointee-typed value. Copying the parameter works.
    Pinned by `conformance/probes/mut_pointer_parameter_reassign.mojo`.
  - `comptime-if-dropped-branch`: a type error inside the untaken branch of a
    `comptime if` is never reported, because elaboration drops the branch
    before the checker runs. Upstream type-checks every branch symbolically
    and rejects the program (`var x: Int = "hello"` in the untaken branch, and
    `x.nonexistent()` on a `T: Copyable` parameter). Section 3's first task is
    the fix.
  - `comptime-if-module-level`: a `comptime if` at module level is accepted,
    while upstream requires one inside a function (`'comptime if' must be
    contained in a function`). `assets/ok/generic_ctfe_value_param.mojo` and
    `assets/ok/generic_ctfe_associated_value.mojo` use it as a compile-time
    assertion.

  Three divergences are retained on purpose and re-probed rather than fixed;
  they are listed in [`docs/non-goals.md`](non-goals.md).

  Model: Opus per row, except four that change how the checker decides
  rather than what it spells — `partial-field-move-parent-used` (ownership
  goes from field-wise to whole-parent), `symbolic-origin-pointer-write`
  (origin arguments must survive checked identity, which erases them today),
  `assign-view-over-source` (owned-interior origins on the `String` view
  methods, checked against the assignment destination), and
  `comptime-if-dropped-branch` (section 3's first task). Those four are
  Fable. `ref-binding-register-value` is an Opus change with wide fixture
  fallout, so it wants its own pass.

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
  - Number formatting in Rust: the VM's `Display for Value` and the native
    `mjrt_fmt_i64` / `mjrt_fmt_u64` / `mjrt_fmt_f64`. Upstream formats
    `Int` and `Float64` in Mojo (`_write_int`, Dragonbox in
    `format_float`).
  - `mjrt_repr_string` for the native string repr. The VM side is already
    Mojo (`String.write_repr_to`).
  - `mjrt_pow` for integer `**`. Upstream's `Int.__pow__` is Mojo.

  Model: Opus per port, except Dragonbox float formatting, which is Fable
  — a from-scratch algorithm whose output every fixture compares against.

  Four runtime services are deliberately not on that list; they are in
  [`docs/non-goals.md`](non-goals.md).

- [ ] **Eight `assets/ok` fixtures do not compile with the pinned Mojo**

  Problem: the ordinary `assets/` folders may hold only programs the pinned
  Mojo compiles, and a sweep of the files containing `comptime if` found eight
  that it rejects. None of the eight is in `conformance/cases.tsv`, so nothing
  catches them.
  - `is_same_type[T, U]()` does not resolve upstream at all
    (`type_predicate_comptime_if.mojo`, also `stdlib/std/algorithms.mojo`).
  - The `IsTrivially*` predicates need `from std.traits import …` upstream
    (`bool_alias_predicate_where.mojo`, `maybe_uninit_predicates.mojo`,
    `typelist_comptime_vocabulary.mojo`).
  - `from std.collections.tuple import Tuple` does not resolve upstream, and a
    struct parameter must be spelled `Self.Ts` in a method signature
    (`variadic_method_type_params.mojo`,
    `variadic_pack_upstream_spellings.mojo`).
  - Module-level `comptime if` (`generic_ctfe_value_param.mojo`,
    `generic_ctfe_associated_value.mojo`).
  - Decide per fixture: respell it, or move it under `assets/extensions/` with
    a recorded reason.
  - The sweep covered only files containing `comptime if`. The rest of
    `assets/` has not been checked the same way.
  - Model: Opus. Oracle-driven respelling, one fixture at a time. Only the
    module-level `comptime if` pair needs a judgment call.

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
- Sections 1 and 2 end each checkbox with a **Model:** bullet — a quick
  complexity and blast-radius estimate saying which model should take the
  task. Fable for work that changes a contract, spans phases, or has no
  named lever; Opus where the site and the rule are both known.

## Working Rule

1. Start with a self-hosted library or small user-facing acceptance case.
2. Record the current failure with a focused test.
3. Implement the smallest compiler change that makes the program honest.
4. Add positive and negative coverage at the owning compiler phase.
5. Run `scripts/check` before marking the task complete.
