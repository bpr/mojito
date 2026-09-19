# Mojito Roadmap

The single task tracker: an ordered checklist of **unfinished** work we
intend to do. Two kinds of entry leave this file. Completed work goes to the
supported surface in [`docs/features.md`](features.md), user-visible history
in [`CHANGELOG.md`](../CHANGELOG.md), and lasting design invariants in
[`docs/architecture.md`](architecture.md). Work we have decided *not* to do —
deferred options, capabilities kept on purpose, and limits that match
upstream — goes to [`docs/non-goals.md`](non-goals.md). North star:
self-hosting, and an implementation that resembles Mojo's own as closely as a
small compiler can — prefer the smallest honest language change that unlocks a
real library pattern, with positive and negative tests. The architectural
distance from Mojo is a gap to close in stages
([`docs/architecture.md`](architecture.md)), not a divergence we have
accepted.

Sections and their checkboxes are in implementation order; the first unchecked
box is the default next task. *(recurring)* and *(any order)* sections are
exempt from the ordering.

Sections 2 and 3 are ordered differently: every checkbox there, and every
bullet inside a checkbox that holds more than one independent item, names the
model that should take it. Each Opus entry also says whether it can be started
as-is or wants a plan first, and they are sorted Opus as-is, then Opus plan
first, then Fable. Where a
strict dependency forces a different order, the entry says so.

Sections 1 to 3 are ordered by what the Pliron direction in
[`docs/pliron-future.md`](pliron-future.md) depends on. Section 1 does not wait
for sections 2 and 3 to empty, since section 3 reopens at every re-pin; the
sections can interleave.

## Ordered Work

### 1. Front-End Groundwork For A Pliron-Centered Architecture

The assessment is [`docs/pliron-future.md`](pliron-future.md): Pliron cannot
give Mojito Mojo's shape on its own, because Mojo type-checks parametric code
before instantiating it while Mojito elaborates first and checks the clones.
These tasks fix that order. The first step landed as source validation
(`checker/comptime_validation.rs`): every `comptime if` arm and `comptime for`
body is checked with the declaration's parameters symbolic before elaboration
selects, and `rebind[Dest](value)` is implemented. The second step took the
explicit-destruction analysis with it: every parametric body is now judged
with its own parameters symbolic, whatever its call sites do, because a
bound-generic template survives beside its specializations and validation runs
the analysis over compile-time-keyed bodies. The third step made a `rebind`
key specialization the way a `comptime if` does, so its assertion is made on
every instantiation and never against a symbolic parameter. Each task pays off
by itself, and together they are what moving toward Mojo's shape needs.

Unlike sections 2 and 3, this section is sorted in **dependency order**: each
entry names what it depends on, and an entry with no dependency sits as early
as its size allows. The **Model:** bullet carries the complexity estimate and
says whether the entry can be done as-is or must be planned first.

- [ ] **An overloaded compile-time-keyed `def` cannot be called without its
  parameters**

  Problem: two `def kind[T: Copyable]` overloads whose bodies hold a
  `comptime if` run at the pin (`kind(3)`, `kind(3, 4)`), while Mojito
  reports "compile-time call arity: generic 'kind' requires compile-time
  parameter 'T'".
  - The three template classes are keyed by name and admit a name only when
    it is unique (`def_name_counts`, `comptime/comptime.rs`), because
    overload selection belongs to the checker. An overloaded compile-time-keyed
    `def` therefore joins no class: it is neither retained as a bound generic
    nor served by the specialization request path.
  - Found while making a `rebind` key specialization: those bodies joined the
    same class and inherited the limitation.
  - Depends on nothing.
  - Model: Opus, plan first. The plan must say how a request names one
    overload of a template.

- [ ] **A compile-time-keyed `def` cannot be passed as a function value**

  Problem: `apply(as_int, 3)`, where `as_int[T]` holds a `comptime if` or a
  `rebind` and `apply` declares a callable bound, runs at the pin and reports
  "Undefined variable 'as_int'" in Mojito.
  - The template is dropped or stubbed, and only its `$`-mangled clones carry
    a name; a bare reference resolves to neither.
  - The rejection is safe (no wrong answer), but the message names nothing
    the source wrote.
  - Depends on nothing.
  - Model: Fable, plan first. The plan must say which clone a bare reference
    names, and what the rejection says when none can be chosen.

- [ ] **A `def`'s own type pack cannot be queried in a runtime position**

  Problem: `return 1 + Us.length` fails with "Undefined variable 'Us'", while
  the pinned Mojo runs it.
  - A free `def count[*Us](var *extra: *Us)` and a method-own pack on any
    struct fail the same way.
  - A compile-time position (`comptime for i in range(Us.length)`) works, and
    so does a variadic struct's own pack (`Self.Ts.length`) in a runtime
    position.
  - Only `generate_struct_spec` binds a pack into the substitutions that
    `fold_pack_typelist_use` (`comptime/rewrite.rs`) reads during
    materialization. `def` specialization never does.
  - Pinned by `conformance/fixtures/pack_length_runtime_position.mojo`
    (`pack-length-runtime-position`, `mojo-only`).
  - Depends on nothing. It stays in the elaborator, so it does not wait for
    the symbolic pack work below.
  - Model: Opus, plan first. The binding site is known, but the plan must
    enumerate the clone paths that share it (free `def`, method-own pack,
    nested forwarding) and the materialization rewrite each one runs.

- [ ] **A pack-keyed template body is not validated symbolically**

  Problem: source validation checks a `comptime if` arm only where the
  checker can bind the declaration's parameters; a body keyed on a variadic
  pack (a variadic struct's methods, a method-own or free `*Ts`) still checks
  only per instantiation, so an untaken arm there is still never reported.
  - `conformance/fixtures/pack_element_type_narrowing.mojo`
    (`pack-element-type-narrowing`, `mojito-only`) pins the visible half:
    inside a folded `comptime if Self.Ts[i] == T` arm Mojito still reads
    `self.storage[i]` as a `T`, where upstream keeps `Ts.values[i]` and
    demands `rebind[T](...)`. `assets/ok/pack_element_rebind.mojo` is the
    spelling both compilers accept.
  - The lever is an opaque pack element in the checker: `Self.Ts[i]` under a
    symbolic index becomes a `Ty::Param` whose bounds are the pack's declared
    bound plus the enclosing method's `conforms_to(Self.Ts.values, …)` /
    `all_conforms_to` assumptions. `check_def_inner`'s `opaque_params` is the
    existing precedent for one such element.
  - The template-shell boundary to lift is `concrete_only_struct` /
    `concrete_only_def` in `checker/comptime_validation.rs`, and the
    per-instantiation stub path in the elaborator stays as the executable
    route.
  - A validated body that reaches a template-shell member ends validation
    for the whole program without a verdict (`is_template_shell_member_error`),
    so no untaken arm anywhere is reported. The members are those of `Tuple`
    and other shell structs: `Tuple(1, "one")`, `a == b` over tuples,
    `pair.reverse()`.
  - A body reading `reflect[...]` is not validated at all (`validates_body`).
    Lifting the boundary retires both escapes.
  - The same boundary lets such a body write through a by-value `rebind`.
    `check_rebind_place` (`checker/rebind.rs`) exempts every `$`-mangled
    clone, trusting source validation to have judged the template. These
    templates were never validated.
  - Every stdlib pack body passes through it: `Tuple`'s comparison, hashing,
    and writing methods, `Variant`'s `isa`/`unsafe_get` on
    `__VariantStorage[*Self.Ts]`, `TString.write_to`, and
    `FormatStruct.params`/`fields`. Those bodies are the acceptance list.
  - Depends on nothing. Unblocks constructor inference below.
  - Model: Fable, plan first. A new type form, conformance assumptions taken
    from where clauses, and builtin storage operations over a symbolic pack
    span the checker; the plan's job is to keep the stdlib compiling at every
    step.

- [ ] **Parameter expressions have no symbolic form**

  Problem: `CtValue` carries concrete values plus an opaque symbolic `Param`,
  so two parameter expressions can only be compared by making them concrete.
  - `SIMD[dt, n + 1]` and `SIMD[dt, 1 + n]` cannot be judged equal today.
  - Symbolic branch checking over a `DType` or vector width needs this, and
    so does any parametric IR.
  - Upstream stores parameter expressions as uniqued typed attributes and
    decides equality by canonicalization rather than evaluation.
  - Shape the representation like a Pliron attribute, uniqued and
    canonicalized, so a later dialect move is a re-homing and not a redesign.
  - Depends on nothing. It gates the `DType`/vector validation entry and the
    Pliron parametric layer.
  - Model: Fable, plan first. A representation change under `CtValue`,
    `CtExpr`, the checker's constraint evaluation, and the elaborator's
    evaluator, with no single lever. Astra for the plan if it is to be shaped
    as the attribute layer of a future dialect rather than as a checker-only
    normal form.

- [ ] **A `DType`- or vector-keyed template body is not validated symbolically**

  Problem: a body keyed on a `DType` parameter or a vector-typed value
  parameter checks only per instantiation, because `Ty::Simd` holds a
  concrete element type and width, so an untaken arm there is still never
  reported.
  - The same boundary hides a second gap: a validated body that applies a
    `DType`-keyed struct (`Vec[DType.float64](...)`) reaches the template
    shell where the elaborator would have minted the instance.
  - The lever is a symbolic element type and width on `Ty::Simd`, and the
    scalar-range family (`std/range.mojo`) plus the SIMD-keyed stdlib
    methods are the fallout; `dtype_keyed`/`concrete_only_def` mark the
    boundary today.
  - Depends on the symbolic parameter-expression form above (a width is a
    parameter expression) and on the pack-keyed entry's opaque-element
    machinery.
  - Model: Fable, plan first. Changes the SIMD type's contract across the
    checker, the elaborator's width folding, and native lowering's vector
    types.

- [ ] **A variadic struct's type arguments are not inferred from its
  constructor**

  Problem: `Pair((1, True))` for `struct Pair[*Ts](...)` with
  `var storage: Tuple[*Self.Ts]` runs at the pin, while Mojito requires
  `Pair[Int, Bool](...)`
  (`assets/type_error/pack_struct_needs_explicit_args.mojo`, a `divergence`
  row of `conformance/assets-mojo-errors.tsv`).
  - Monomorphization (`crates/mojito-comptime/src/comptime/mono.rs`) runs
    before type checking, so argument types are only syntactically known
    there.
  - Inference needs a checker-owned instantiation: the checker must type the
    template's constructor symbolically and unify `*Ts` against the
    argument types, then request the instance the way it already requests
    inferred bound-generic clones.
  - Depends on the pack-keyed validation entry (the template must type
    symbolically before its constructor can be unified against).
  - Model: Fable, plan first. Moves one instantiation decision from the
    elaborator to the checker and touches the discovery loop's request kinds.

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
  - Depends on the symbolic parameter-expression form above, which is the
    parametric layer [`docs/pliron-future.md`](pliron-future.md) found missing
    from the pivot plan's dialect design. Last in this section.
  - Model: Astra for the plan and the measurement design (the broadest scope
    in this document: it reopens the waist and the dialect policy), Fable to
    build and measure the A1 slice once planned.

### 2. Native Backend

Sorted Opus as-is, Opus plan first, then Fable (see **Entry Style**). The
ABI-bump collector is last
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

- [ ] **Operator operands and the bundled `hash` body take lifecycle copies**

  Problem: `b == c` on a struct whose copy constructor prints prints
  `copy` for each operand, and `hash(b)` prints it twice, on both backends.
  - Upstream's `__eq__(self, other: Self)` and `__hash__` borrow their
    operands; nothing is copied.
  - A named binary-operator operand lowers as a `var.use` copy: the
    checker's `borrow_nominal_place_argument` mark, which `len`, `print`
    and the conversion builtins record, is missing on the operator path.
  - `hash(b)` borrows at the call site; the copies happen inside the
    bundled generic `hash[T: Hashable, ...](value: T)` body in
    `stdlib/std/hashlib/hash.mojo`, so that body (or the `T`-typed
    parameter read it makes) is the second lever.
  - Probe first: a `Box` owning a `List[Int]` with a printing
    `__init__(out self, *, copy: Self)`, compared `==` and hashed,
    verified against the pin.
  - Model: Opus, plan first. One recording site plus one stdlib body, but
    the plan bounds the fixture fallout of any `__eq__` that relied on the
    copy.

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
  - Native `input()` never raises `EOF`. `mjrt_read_line` writes an empty
    `MjString` both for an empty line and at end of input, so the lowering
    cannot tell them apart. The checker and the VM already raise `EOF`, as
    upstream does.
  - The fix is a `mjrt_read_line` result that reports end of input, and a
    raising `lower_input_builtin` that turns it into `Error("EOF")`.
    `assets/ok/pliron_input_echo.mojo` catches the error, so its text is the
    same on both backends in the meantime.
  - A later task that needs an ABI bump joins this entry rather than
    getting its own.
  - Model: Fable. The signature classification changes the native calling
    convention itself, and the ABI version, `docs/native-abi.md`, the
    runtime, and the parity harness all move together.

### 3. Catch Up To Current Mojo *(recurring — reopens at every nightly re-pin)*

- When the pinned nightly moves: re-pin [`docs/mojo-nightly.md`](mojo-nightly.md),
  re-probe [`conformance/parity.tsv`](../conformance/parity.tsv), and burn
  down the divergences (`parity.tsv` notes, and the `mojito-only` /
  `mojo-only` rows of `conformance/cases.tsv`).
- Rule: Mojito matches or subsets Mojo. An extension is *kept* only when it
  tracks an announced upstream direction (today: direct `ref` struct
  fields), is listed in [`docs/non-goals.md`](non-goals.md), and is
  re-probed at every re-pin; every other Mojito-only acceptance is a
  divergence on the ledger below, waiting to be withdrawn. Both keep their
  fixtures under `assets/extensions/`.
- The `a79fbdf59f2` pass (2026-08-26, Mojo `1.1.0.dev2026082605`) is
  complete (`docs/mojo-nightly.md`). The next re-pin recreates this
  section's checkbox.

The three checkboxes below, and the bullets inside the two standing ones,
are sorted Opus as-is, Opus plan first, then Fable (see **Entry Style**).

- [ ] **A parametric nested `def` named as a value reports its marker**

  Problem: `apply(inner, 3)` and `var g = inner`, for a nested
  `def inner[U: Copyable]`, report `Undefined variable
  'main$110$198$nested$0$inner'`. The pin rejects both too, so the verdict is
  right and only the message is wrong.
  - The lexical pass renames the declaration to its marker and rewrites every
    reference to it, then deletes the template it could not instantiate,
    leaving the renamed value reference dangling.
  - The pin's texts are "cannot use parametric function as a runtime closure"
    for the binding and an `invalid call to '__call__'` for the argument.
  - Pre-existing for a nested `def` the pass already registered; the nested
    compile-time-keyed work widened the class it reaches.
  - Model: Opus, as-is.

- [ ] **Explicit type arguments on a static method of a non-parametric
  struct are rejected**

  Problem: `P.plain[Int](4)` on `@staticmethod def plain[T: Writable](x: T)`
  prints `1` at the pin, while Mojito reports "Undefined variable 'P'".
  - Any generic static on a struct without parameters is affected, whether
    or not its body holds a `comptime if`.
  - The inferred spelling `P.plain(4)` works.
  - The explicit spelling parses as `Invoke` over `Member(P, plain)`. The
    error is raised before the non-parametric static path in
    `checker/method_calls/mc_infer.rs` sees the call.
  - Model: Opus, plan first. The plan must first find which pass infers the
    bare type name as a value.

- [ ] **A call through a `ref` to a callable value is rejected**

  Problem: `for f in fns: print(f(5))` and `ref g = fns[0]; print(g(1))` run at
  the pin over a function display, while Mojito reports "'f' has type ref
  def(Int) thin -> Int and is not callable".
  - Both thin and capturing elements are affected.
  - The indexed call `fns[0](5)` already works through element-call dispatch.
    A `ref`-typed callee has no such path.
  - Model: Opus, plan first.

- [ ] **A view returned by a method on a `List` element reads freed memory on
  the VM**

  Problem: `var v = ys[0].rstrip()` followed by `String(v)` fails with "use
  after Pointer deallocation", where the pin prints the stripped text.
  - The element receiver is an `Index` place: the view-result borrow in
    `aggregate_origins` (`checker/origins/ref_params.rs`) lends only
    identifier and field receivers, and MIR materializes only non-place
    receivers, so nothing keeps the element's bytes alive for the view.
  - The call-result aliasing rule already projects such a view through the
    element place (`carried_argument_origins`), which is the origin the
    borrow should carry.
  - Pinned by `conformance/probes/list_element_view_method_result.mojo`.
  - Model: Opus, plan first. Lending an element place changes loans for every
    view-returning method on a subscript, so the plan enumerates that
    fallout first.

- [ ] **A tuple binding with a `Tuple[...]` annotation loses `reverse` and
  `concat`**

  Problem: `var t: Tuple[Int, String] = (1, "x")` followed by `t.reverse()`
  or `t.concat(Tuple(True))` runs at the pin, while Mojito reports "type
  'Tuple$t2[y3:Inty6:String][Int, String]' has no method 'reverse'".
  - The same calls work on an unannotated binding (`var t = (1, "x")`) and
    on `var t = Tuple(1, "x")`.
  - The annotation resolves to the minted `Tuple$t2[...]` struct, so method
    lookup reports the missing member before the builtin tuple path
    (`infer_tuple_method`, reached from `method_calls/mc_infer.rs`) or a
    `TupleTransformRequest` for the clone is ever considered.
  - Found in the 2026-09-17 gate triage; no fixture pins it yet.
  - Model: Opus, plan first. The plan decides whether the annotation should
    keep the public `Tuple` spelling or the lookup should fall through, and
    lists the other members the minted spelling hides.

- [ ] **A reflection method call in a runtime position is rejected**

  Problem: `comptime r = reflect[Point]` followed by `print(r.field_count())`
  prints `2` at the pin, while Mojito reports "Undefined variable 'r'".
  - Binding the result first works: `comptime count = r.field_count()` then
    `print(count)`.
  - The reflection handle erases before the executable check, so a method
    call on it must fold to its compile-time value where it stands, as the
    upstream materialization of an `Int` result does.
  - A direct `reflect[Point].field_count()` in a runtime position fails too,
    with "type 'reflect[…]' has no method 'field_count'".
  - Found in the 2026-09-17 gate triage; no fixture pins it yet.
  - Model: Opus, plan first. The plan lists which handle results are
    implicitly materializable (an `Int`, a `Bool`) and which still need an
    explicit crossing (a name list).

- [ ] **`reflect[T]` over a function's type parameter is rejected**

  Problem: `def f[T: AnyType]()` holding `comptime r = reflect[T]` and
  `comptime count = r.field_count()`, called as `f[Point]()`, prints the
  field count at the pin, while Mojito reports "not a compile-time value:
  'T' is not a compile-time type".
  - The error fires even when `f` is never called, so the unspecialized
    template body is evaluated with `T` unbound.
  - The failing evaluation is `reflect[...]` in `comptime/eval.rs`, through
    `param_arg_type`. A template body should defer it the way a `comptime
    if` on `T` becomes a per-instantiation stub.
  - `reflect[Point]` over a concrete type in a value-parameterized
    `def f[n: Int]()` already works.
  - Found in the 2026-09-17 gate triage; no fixture pins it yet.
  - Model: Opus, plan first. The plan names every elaborator path that
    evaluates a retained template body.

- [ ] **Constructing a struct's own type parameter fails in every instance
  clone**

  Problem: `self.x = Self.T()` in a method of `struct Box[T: Copyable &
  Deinitable & Defaultable]` is rejected for the instance — "in 'reset'
  instantiated for 'Box[Float64]': type mismatch for assignment target:
  expected Float64, found T" — while the pin runs it.
  - A clone respells `Self.T` in its annotations but not in a construction
    expression, so the call still types as the template's `T`.
  - A constructor clone fails the same way, so the shape reaches lifecycle
    and ordinary bodies alike.
  - Found while closing the overloaded-constructor-family item; no fixture
    pins it yet.
  - Model: Opus, plan first. The plan names the substitution
    `specialize_method_clone` applies to expressions, not only to
    annotations.

- [ ] **An exact constructor overload loses to its generic sibling as
  ambiguous**

  Problem: `Tag[Int](3)` on a struct declaring `__init__(out self, n: Int)`
  beside `__init__(out self, n: Self.T)` prints `int` at the pin, while
  Mojito reports "invalid call to 'Tag': ambiguous overloaded constructor
  call".
  - Upstream picks the more specific signature. Mojito's construction
    selection scores the substituted `Self.T` parameter as an equal match
    (`select_method_overload`, `checker/declarations.rs`).
  - The preference already exists one level down: a per-call clone of a
    generic constructor wins over the template it was minted from
    (`constructor_is_concrete`).
  - Such a family is also the one shape that collapses on the instance, so
    its constructor clone family stays withdrawn.
  - Found while closing the overloaded-constructor-family item; no fixture
    pins it yet.
  - Model: Opus, plan first.

- [ ] **`repr` of a sized scalar omits or misstates its type name**

  Problem: `repr(Float32(0.5))` and `repr(Int8(3))` print `Float32(0.5)` and
  `Int8(3)` at the pin, but `0.5` and `3` on the VM, and `Float64(0.5)` and
  `3` natively.
  - The VM's `scalar_repr` (`backend/vm/dispatch.rs`) labels only `Int`,
    `UInt`, and `Float64`, so a `SIMD` width-1 value falls through to its
    display.
  - Pliron's `lower_repr_builtin` (`lower/methods.rs`) labels every float
    scalar `Float64(`, `Float16` and `Float32` included.
  - The label is the scalar alias (`Dtype::scalar_alias`); the value text
    stays the float-format divergence the Dragonbox item owns.
  - Model: Opus, as-is.

- [ ] **Upstream `DType` names with no Mojito dtype are rejected**

  Problem: `print(DType.uint128)` runs at the pin (`uint128`), while Mojito
  reports "DType.uint128 is not supported yet"
  (`assets/type_error/dtype_upstream_only_rejected.mojo`, a `divergence` row of
  `conformance/assets-mojo-errors.tsv`).
  - The names are `uint`, `bfloat16`, `int128`, `uint128`, `int256`,
    `uint256`, the `float4`/`float6`/`float8` families, and `_uint1`/`_uint2`/
    `_uint4` (`UPSTREAM_ONLY_DTYPE_NAMES` in `mojito-ast`).
  - `UInt.dtype` is `DType.uint` and rejects the same way
    (`assets/type_error/dtype_uint_alias_rejected.mojo`, the `simd_dtype`
    arms in `checker/indexing.rs` and `associated_value` in
    `comptime/elab.rs`).
  - `Dtype::float_query` answers only `float16`/`float32`/`float64`; a new
    float format adds its row there.
  - Each needs a `Dtype` variant with upstream's code in `Dtype::code`, even
    where no `SIMD` lane of it exists yet, so the value can print and compare.
  - `Float16` set the pattern for a new lane: extend the table methods in
    `mojito-ast`, route rounding through `Dtype::round_lane` and
    `Dtype::float_literal_lane`, and let the build's exhaustiveness errors
    list the rest. The native lowering matches with wildcards, so its sites
    need a manual `rg` pass.
  - Model: Opus, plan first.

- [ ] **Mojito-specific shortcuts to move toward Mojo's shape** *(standing,
  any order)*

  Problem: parts of Mojito's stdlib lean on the Rust runtime where upstream
  is pure Mojo. Each is a candidate port, preferred over any new bridge
  (2026-09-07 direction).
  - Float formatting in Rust: the float arm of the VM's `Display for Value`
    and the native `mjrt_fmt_f64`. Upstream formats `Float64` in Mojo
    (Dragonbox in `format_float`). Five corpus fixtures print different text
    on the two compilers because of it
    (`conformance/assets-mojo-output-diffs.tsv`, family `float-format`): the
    pin writes an exponent sign (`1e+23`, Mojito `1e23`) and renders a
    `Float32` at its own precision (`0.1`, Mojito `0.10000000149011612`).
    - Model: Opus, plan first. Transliterate a permissively licensed Rust
      Dragonbox (MIT or Apache-2.0 — third-party crates are allowed, see
      `AGENTS.md`) into Mojo rather than deriving the algorithm; the plan
      picks the source and pins the shortest-round-trip cases. Only a
      from-scratch derivation would want Fable.
  - `DType` is a compiler builtin (`Ty::Dtype`, with its `is_*` queries and
    display in the VM and the native lowering) where upstream's is a stdlib
    struct over a one-byte code. A bundled `struct DType` needs struct-valued
    associated `comptime` members (`eval_associated_ct` rejects them), runtime
    reads of `StructName.NAME`, struct value parameters on defs, and a bridge
    from a frozen struct value to `Dtype` at every hard-wired `DType` site.
    - Model: Fable, plan first. Several checker and elaborator capabilities
      land before the struct can replace the builtin.

  Four runtime services are deliberately not on that list; they are in
  [`docs/non-goals.md`](non-goals.md).

- [ ] **`String` always owns a heap buffer, where upstream's has three
  representations**

  Problem: `String(literal)` allocates and copies the literal's bytes, while
  upstream points at the static bytes and copies only on the first mutation.
  - Upstream packs a static-constant, an inline (up to 23 bytes), and a
    reference-counted heap form into the same 24 bytes, flagged in
    `_capacity_or_data`; Mojito's `{data, size, cap}` has only the heap form.
  - No program output differs; allocation counts and `capacity()` do.
  - Port the static form first: every mutator, `__del__`, copy, and move
    must respect a non-owning flag, on both backends.
  - Model: Opus, plan first.

- [ ] **Behavioral divergences from the pinned Mojo — burn to zero**
  *(standing)*

  Every new divergence lands here with a probe or a `cases.tsv`
  `mojito-only` / `output-diff` row, and leaves when its probe promotes to
  an `assets/ok` fixture.

  Open today:
  - `result-alias-rule-coverage`: a free function whose return declares an
    owned interior of an argument is not judged by the call-result aliasing
    rule (`checker/origins/result_alias.rs`), so `w = keep(view_x(w))` runs
    in Mojito and is rejected upstream.
    - `view_x(v: W) -> StringSpan[origin_of(v.x)._get_owned_interior["bytes"]]`
      carries its argument's origins unprojected, because only methods record
      `view_result_interiors`; a free call needs the same side table keyed
      by the projected parameter.
    - Model: Opus, plan first. Free-function signatures keep no source return
      type the call site can read, so the plan picks where the parameter
      projection is recorded.
  - `unpack-assign-call-over-viewed-local`: `a, b = pair(a.rstrip())` runs
    upstream (`ab 1`), while Mojito rejects it with "access to 'a' conflicts
    with live reference '$arg_loan_r5'": the argument's view anchor outlives
    the call into the unpacking store.
    - Model: Opus, plan first. The anchor's statement-end keep-alive is what
      every other call argument relies on, so shortening it for unpacking
      needs its fallout checked first.
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
  - `native-arithmetic-edge-cases`: seven corpus fixtures compute different
    numbers from the pin, found by the 2026-09-12 stdout sweep and listed in
    [`conformance/assets-mojo-output-diffs.tsv`](../conformance/assets-mojo-output-diffs.tsv)
    under `arithmetic`. The clearest is a shift past the bit width
    (`assets/ok/pliron_straightline.mojo`: `a << 65`, where the pin prints 7
    and Mojito 51102306), and the SIMD shift and floor-division fixtures
    disagree wholesale. Also here: an out-of-range float-to-int cast and `Float64` `**`.
    `round()`'s half-way case left this list on 2026-09-13 when `round`
    became ties-to-even on both backends.
    - Model: Opus, plan first. Each case needs the pin's rule established
      before Mojito's is changed, and `docs/native-abi.md` already defines
      some of them deliberately (wrapping overflow), so the plan decides
      which are bugs and which are recorded choices.
  - `native-exclusions-returned`: `assets/ok/lambda_hof.mojo` is the one
    fixture the native backend still rejects — "unsupported binding call to
    `main$scale` during monomorphization: Missing("x")".
    - `observe[f: def(x: Int) capturing[origins] -> Int]` takes its callable
      as a compile-time parameter and the argument captures `factor`.
      Monomorphization binds the callable by name and rewrites the body's
      read of it to `Const::Function("main$scale")`
      (`mono/substitute.rs`), which erases the environment; a capturing
      lambda's lowered body takes its captures as leading parameters, so the
      direct call is one argument short.
    - The VM keeps the closure and calls it indirectly. Passing the
      environment natively means the instance takes the closure as a runtime
      argument, which changes the instance's arity — something
      monomorphization never does today. Both existing guards (the
      `CallIndirect` rewrite's "generic retained callable … has captures",
      and `infer_call`'s captures-empty condition) are simply not reached,
      because the constant-folding happens first.
    - The other three exclusions of the 2026-09-18 regeneration closed on
      2026-09-19: an instance key that kept its arguments' origins minted a
      second instance under the same origin-free symbol
      (`interior_dest_rebind_releases_loans`,
      `nominal_string_bytes_capacity`), and a direct
      `iterator.__next__()` through the contract had no native
      `CopyIteratorReference` adapter, which a `for` loop's own advance has
      had all along (`copyable_iterator_reference_aggregate`).
    - Model: Opus, plan first. The arity change touches the instance
      signature, the call site, and the pliron parameter-argument rule
      together.
  - `destructor-timing-against-the-pin`: two corpus fixtures run the same
    destructors later than the pin does
    (`conformance/assets-mojo-output-diffs.tsv`, family `drop-timing`):
    `assets/ok/owned_pointer_api.mojo` and
    `assets/ok/try_region_drop_timing.mojo`. The two `MaybeUninit` fixtures
    that used to skip destructors outright left this list on 2026-09-17, when
    owning temporaries gained their hidden slots.
    - Model: Opus, plan first. Both remaining cases are orderings rather than
      omissions, and the plan establishes where the pin runs each destructor
      before Mojito's schedule is moved.
  - `pointee-element-reference-return`: a method returning a reference into
    what a `Pointer[T, Self.o]` field borrows names the region with the
    struct's own origin parameter. The pin distinguishes the element
    sub-origin (`origin_of(o["element"])`) and wants
    `ref[origin_of(self.src[][i])]`, which Mojito's escape check rejects
    because no `Origin` variant carries a projected `SelfParam`. Two
    `assets/origin_ok` fixtures moved for it, and five `Pointer` iteration
    twins (`assets/extensions/ok/pointer_field_reference_yielding_iteration*`,
    `pointer_field_comprehension_borrowed_named_source`,
    `pointer_field_parametric_mut_iterator_read`) joined their `ref`-field
    originals.
    - Model: Opus, plan first. The fix adds a projected receiver origin to
      `mojito_types::origin::Origin`, which MIR text, verification, and
      substitution all read.
  - `local-comptime-capture`: a function-local `comptime` constant read from
    a nested `def` is an ordinary immutable local in Mojito, so it must be
    named in the capture list; the pin treats it as a compile-time constant
    and rejects naming it there. Mojito cannot simply stop requiring the
    capture: the constant has real storage in the outer frame and the lifted
    function has no binding for it.
    - Model: Opus, plan first. The fix makes a folded `comptime` local a
      constant the lifted body can read, which is a lowering change, not a
      scope-rule change.
  - `capturing-lambda-argument` / `capturing-lambda-locals` /
    `owned-capture-closure-locals` / `mut-self-callable-struct`: four
    callable-shape leniencies. Mojito passes a capturing lambda as a runtime
    argument, binds one to a local under an explicit `capturing[...]`
    annotation, and accepts a `mut self` `__call__` as a `def(...)`
    conformer. The pin takes a capturing callable only as a compile-time
    parameter (and refuses a lambda even there), types a capturing lambda as
    a plain `def(...) -> T` that converts to nothing, and wants a read
    receiver. Three corpus fixtures moved to `assets/extensions/`; three more
    were respelled onto `thin` contracts and `@parameter def` arguments.
    - Model: Opus, plan first. The four share one question — what a capturing
      callable *value* is — and the plan settles that before any rejection.
  - `interior-generation-view-consume` / `interior-generation-view-drop`:
    a struct field typed
    `Pointer[T, Self.origin._get_owned_interior["tag"]]` over a generic
    origin parameter. The pin parses the projection but calls the interior
    reference never-initialized, so the carrier struct does not type-check
    there at all. Two `assets/ownership_ok` fixtures moved for it.
    - Model: Opus, plan first. Upstream's owned-interior origins are real;
      what differs is which structs may name one, so the plan probes that
      rule before Mojito narrows.
  - `iterable-element-identity`: Mojito's `for` yields the *iterable*'s
    `Element`, so one associated type serves a generic signature and the
    loop; the pin yields the *iterator*'s `Iter.Element` and will not convert
    between the two without an identity clause Mojito does not implement.
    - `assets/extensions/ok/iterable_associated_element.mojo` has no `main`:
      its `for` over a trait bound that declares only `Element` never
      compiled upstream, where even a real `Iterable` bound abandons its
      `AnyType` iterator temporary.
    - Model: Opus, plan first.
  - `mojito-only-stdlib-algorithms` / `owning-family-container-apis`: two
    stdlib surfaces upstream does not have — `std.algorithms`,
    `std.collections.string_dict`, and the owning-family container APIs
    (`deinit_with`, `clear_with`, displacement-returning `insert`). Two
    corpus fixtures moved to `assets/extensions/` for them.
    - Model: Opus, plan first. Whether these leave or stay is a stdlib-shape
      decision, not a respelling.
  - `partial-move-join-imprecision`: a conditional partial move on one
    branch joined with a whole move on the other is accepted, though the
    first path reaches the exit with a hole the pin rejects (`field 'p.a'
    destroyed out of the middle of a value`).
    - The three-point move lattice joins `a: MaybeMoved` under an intact
      base with a wholly moved base into a state it cannot tell from
      intact-or-wholly-moved.
    - Pinned by `conformance/probes/partial_move_join_imprecision.mojo`.
    - Model: Opus, plan first. A fourth lattice point, or a per-node
      "may hold a hole" flag that survives joins, is the lever; the plan
      picks one.
  - `assign-plain-span-argument-over-list`: `xs = rebuild(Span(xs))` runs
    upstream (`1`) and is rejected in Mojito with "access to 'xs' conflicts
    with live reference 'xs'".
    - The temporary argument's anchor now ends before the store, as for the
      passing `s = String(StringSpan(s))`, but the assigned `List[Int]`
      result still records a loan on `xs`, so the store conflicts with the
      new value itself.
    - Pinned by `conformance/probes/assign_plain_span_argument_over_list.mojo`.
    - Model: Opus, plan first. Where the result's loan comes from (MIR
      `aggregate_borrows` or a replayed transfer effect) is not yet known.
  - `named-tuple-unpack-copy`: unpacking a named Tuple with a heap element
    shares the element with the source, so the VM frees it twice.
    - `var first, second = pair` over `Tuple[Int, String]` prints `3 seven 3`
      at the pin and fails with "use after Pointer deallocation" in Mojito.
    - A `List[Int]` element is rejected at the pin ("cannot be implicitly
      copied"), while Mojito accepts it and fails the same way.
    - The unpack plan in `crates/mojito-checker/src/checker/statements.rs`
      reads each element through the place accessor without the
      implicit-copy check or a copy.
    - Pinned by `conformance/probes/tuple_unpack_named_place_copies.mojo`.
    - Model: Opus, plan first. The lever is the ImplicitCopy funnel, but the
      MIR and VM fallout of copying at the unpack is not enumerated.
  - `ref-field-return-origin-widening`: a view struct that stores its source
    in a direct `ref` field still widens a returned field view to a declared
    `origin_of(self)`.
    - `def view(ref self) -> View[origin_of(self)]` returning
      `View(source, 0)` over `ref source = self.items` passes when `View`
      holds `ref[o] List[Int]`.
    - The `Pointer`-field twin rejects with upstream's "cannot implicitly
      convert 'View[origin_of(self.items)]' value to 'View[origin_of(self)]'"
      (`assets/type_error/return_origin_widening_field_view.mojo`).
    - Why the tail escapes `reconcile_return_origin_tails`
      (`checker/origins/solve.rs`) is not yet known; the likely lever is the
      tail the `ref`-field constructor binds.
    - Pinned by `assets/extensions/ok/ref_field_view_for_temporary.mojo`,
      `ref_field_view_ref_yield.mojo`, `ref_field_view_method_return.mojo`,
      `ref_field_drain_mut_method.mojo`, and `ref_field_chained_view_call.mojo`.
    - Model: Opus. The rule already exists; the gap is one constructor path.
  - `string-subscript-element`: `s[0]` on a `String` yields a character
    upstream and an `Int` in Mojito
    (`assets/ok/nominal_string_indexing.mojo`, `h` against `104`). Two
    smaller text divergences ride along in the same manifest under
    `one-off`: the raised `DictKeyError` renders differently
    (`assets/ok/self_hosted_dict.mojo`), and reflection prints
    `<unprintable>` upstream where Mojito prints the element types
    (`assets/ok/type_names_applied_elements.mojo`).
    - Model: Fable. The subscript's element type is a declaration change in
      the stdlib's `String`, and the other two are texts to match.
  - `int-true-division`: `Int / Int` is true division into `Float64` in
    Mojito; the pin truncates back to `Int` and divides only an `IntLiteral`
    pair into a float. An `output-diff` row, not a rejection, so it is not on
    the burn-down.
    - Model: Fable. The result type of one operator changes, and every
      fixture and stdlib body that divides integers moves with it.
  - `simd-infix-comparison`: Mojito's `<`/`<=`/`>`/`>=` are elementwise at
    every width and its `==`/`!=` compare lane by lane. The pin constrains
    the strict inequalities to `Scalar` and gives `==`/`!=` whole-vector
    meaning, pointing at `SIMD.lt(...)`, which Mojito now has (2026-09-13)
    along with `le`/`gt`/`ge`/`eq`/`ne`. Six corpus fixtures were respelled
    onto the methods.
    - Model: Fable. Withdrawing the infix spelling is a leniency to remove,
      with fallout across the stdlib's SIMD bodies.
  - `simd-element-narrowing` / `simd-inferred-width` / `float-literal-to-int`:
    three small constructor leniencies. Mojito narrows a SIMD element
    argument to the lane type (wrapping an out-of-range literal and a wider
    runtime value), infers an unbound SIMD width from the argument count, and
    truncates a `FloatLiteral` straight to `Int`; the pin wants the lane's
    own scalar, a written-out width, and the `Float64` it truncates from.
    - Model: Fable. Three leniencies to withdraw in one pass.
  - `float32-reduce-ordering`: `reduce_mul` over `Float32` lanes folds left
    at lane precision in Mojito and pairwise in the pin
    (`319256416.0` against `319256448.0` on sixteen lanes;
    `assets/ok/simd_wide_widths.mojo` now reduces an exactly representable
    vector to avoid it). The float-format entry above hides the same rows.
    - Model: Fable. The reduction shape changes in the VM and in the
      `llvm.vector.reduce.fmul` lowering together.
  - `string-span-byte-index` / `string-codepoint-index`: `sp[byte=i]` reads
    the byte value in Mojito and returns the one-byte view upstream, and
    `s[codepoint=i]` is a `Codepoint` in Mojito and a one-codepoint
    `StringSpan` upstream. The same shape as `string-subscript-element`
    above, on the keyword subscripts.
    - Model: Fable. Declaration changes in the stdlib's `String`/`StringSpan`.
  - `split-returns-owned-strings`: `String.split`/`splitlines` return
    `List[String]` in Mojito and owned-interior `StringSlice` views upstream,
    so `var parts = s.split(" ")` then `s = String(parts[0])` runs in Mojito
    and hits upstream's call-result aliasing rejection.
    - Pinned by `conformance/probes/split_returns_owned_strings.mojo`.
    - Model: Fable. An API shape change with display and iteration fallout
      across every `split` caller.
  - `contiguous-slice-result`: a contiguous `List` slice is an owned `List`
    in Mojito and a borrowing `Span` upstream, so only Mojito returns one
    from a `-> List[Int]` function.
    - The call-result aliasing rule rides on it: upstream rejects
      `xs = rebuild(xs[0:1])` because the slice views `xs`'s owned elements,
      while Mojito's copy borrows nothing and runs. Pinned by
      `conformance/probes/list_slice_copies.mojo`.
    - Model: Fable. The return type of `List.__getitem__(ContiguousSlice)`
      changes, and every caller that owns the result moves with it.
  - `int-is-floatable`: Mojito conforms `Int` and an integer literal to
    `Floatable`; upstream conforms neither, so a `Floatable`-bounded helper
    takes only a float there. Mojito also resolves `len(x)` from a bare
    `__len__` where the pin wants a declared `Sized` conformance —
    `assets/ok/dunder_index.mojo` and `assets/ok/self_hosted_vec.mojo` now
    declare it.
    - Model: Fable. Two conformance leniencies to withdraw.
  - `slice-descriptor-kinds` / `unmodeled-struct-decorator` /
    `implicitly-deletable-alias`: three spellings the pin has dropped or
    never had. Mojito splits upstream's single `Slice` into
    `ContiguousSlice`/`StridedSlice` and overloads subscripts on the kind, it
    ignores an unmodeled struct decorator where the pin rejects an unknown
    one (`@value` is now unknown there), and it still normalizes
    `ImplicitlyDeletable` to `Deinitable`, which the pin has removed.
    - Model: Fable. Each is a name or a type to withdraw, with stdlib and
      fixture fallout.

  Five divergences are retained on purpose and re-probed rather than fixed;
  they are listed in [`docs/non-goals.md`](non-goals.md).

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
     - A string element of a compile-time tuple indexed under a `comptime
       for` (`comptime t = (1, "s")`; `comptime for i in range(2)`) breaks
       inside a list display: `first([t[i], t[i]], t[i])` fails MIR
       verification with `register r19 has no checked type`, and `var v =
       t[i]` followed by `first([v, v], v)` with `binding of StringLiteral
       to a slot of type String`. The `Int`/`Bool` twin runs; the
       materialized element keeps its literal type where the display's
       element type has already materialized `String`. Two `var`
       declarations across unrolled iterations also collide (`'v' is already
       declared in this scope`) unless each body opens a block.

  2. **Monomorphization coverage and cost** — where the erased path still
     stands in for an instance clone, and what minting costs.
     - A generic instantiated at `StringLiteral` by a literal argument
       (`T = StringLiteral`) keeps the erased path, since
       `instance_method_clone_name` mints no clone for it. Displays and
       `StringDict` keys now materialize `String`, so they no longer reach
       it; whether `StringDict.__getitem__` can now return a reference
       instead of its `Copyable`-guarded copy is unchecked.
     - A call inside an unstamped bundled body on a bundled struct's
       generic method keeps the erased path. Requests are admitted from
       user code, clone bodies, variadic specs, instances, and user
       structs.
     - An instance clone whose walk cannot resolve an application (a
       variadic template over a nested public `Tuple` argument) is dropped
       to the erased path rather than failing the program. When such a
       method can reach a compile-time-keyed stub, an instance the
       elaborator queued but could not clone rejects the program instead —
       rejecting one the pin accepts. An instance the checker records no
       instantiation for at all (a type parameter inferred as the
       compile-time `StringLiteral`, `Box("a").f()`) is not queued, so it
       keeps the erased path and aborts at run time.
     - Bundled templates keep their constructors on the erased path: a
       `List`/`Dict`/`Optional` instance builds and copies through the
       template's `__init__`/`__copyinit__`/`__moveinit__`, so a `comptime if
       Self.T` there would not fold. User structs clone theirs, and every
       struct's `__deinit__` clone is reached.
     - An erased body still dispatches by runtime name where no checked
       static type reaches it: an operator or protocol dunder called from
       another erased body, a value whose static type is a bare `Ty::Param`,
       an instance reached only from bundled code, and CTFE, which runs
       before any clone exists. A compile-time-keyed method reached that way
       aborts rather than being rejected.
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
       consult the checker's instantiation. Three of the four pieces this
       needed now exist for ordinary nested generics: an unresolved template
       survives `replace_templates` for the discovery check,
       `def_specialization_requests` harvests a nested callee, and
       `NestedMono::scan_expression` consults that request. What remains is
       the checker accepting a nested variadic shell abstractly, and
       softening the pack diagnostics — only an arity failure defers today,
       and a pack failure is `PackBound` or `NotComptime`.
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
     - A lambda parameter annotated `String` (`lambda (s: String): print(s)`)
       reports `unknown type 'String'`, while a nested `def` with the same
       parameter runs. The lambda's hidden `def` apparently misses the
       prelude qualification the other annotations get.     - A list display as an argument to an explicitly applied constructor
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
       behavioral-divergences task of section 3.
     - `range` is invisible in `std.string`.
     - A module loaded while the prelude bootstraps (`std.io` and the whole
       `std.os` graph now) must import `String` explicitly. That graph
       costs Hello World about a second of debug compile time
       (`docs/performance.md`).

- [ ] **Time, random, and testing slices**

  Goal: deterministic testable cores, with host-dependent behavior behind
  runtime services.

- [ ] **Scalars have no comparison methods**

  Problem: `x.ne(y)` on a `Float64` (or any width-1 scalar) is rejected with
  `type 'Float64' has no method 'ne'`, though the pin accepts it.
  - Upstream's scalars are width-1 `SIMD`, so `lt`/`le`/`gt`/`ge`/`eq`/`ne`
    exist on them too. Mojito resolves those methods only on a multi-lane
    `Ty::Simd` receiver (`crates/mojito-checker/src/checker/method_calls/mc_infer.rs`).
  - The methods must keep the multi-lane semantics: the pin's scalar
    `s.ne(s)` is `False` for a NaN (ordered), while infix `s != s` is `True`.
  - Not a wrong answer, only a missing spelling. Until then, `x < y or x > y`
    is the ordered `ne`; infix `!=` answers `True` for a NaN.

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

- [ ] **Naming the bundled stdlib with `-I` breaks every program**

  Problem: `mojito run -I stdlib FILE` fails with `static UnsafePointer
  allocation was removed from Mojo` even for a program that only prints, and
  so do the legacy flat facades (`from list import List`) that `-I stdlib`
  exists to serve.
  - `stdlib/std/memory/alloc.mojo` keeps the one sanctioned
    `UnsafePointer[T].alloc` crossing, allowed only when
    `is_bundled_stdlib_source` in
    `crates/mojito-checker/src/checker/overload_support.rs` says the file is
    bundled.
  - That check, and its sibling `is_bundled_collection_source`, compare the
    source path for byte equality with `bundled_root()`, which is the
    un-normalized `CARGO_MANIFEST_DIR/../..`.
  - A relative `-I stdlib` or a canonical absolute path loads the same file
    under another spelling, so the exemption is lost. Only the literal
    `crates/mojito-module/../../stdlib` spelling runs.
  - The fix is to canonicalize both sides once (or key on module identity
    rather than path). The same comparison gates `--stdlib PATH`, so check a
    copied root with it.
  - The default run with no `-I` is unaffected, which is why
    `tests/flat_stdlib_test.rs` passes.

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
- Sections 2 and 3 carry a **Model:** bullet on every checkbox, and on every
  bullet inside a standing checkbox that holds more than one independent
  item. It is a quick complexity and blast-radius estimate. Fable is for work
  that changes a contract, spans phases, or has no named lever; Opus is for
  work whose site and rule are both known.
- An Opus entry adds "as-is" or "plan first". "Plan first" means the lever is
  known but the fallout is not enumerated yet.
- Those two sections are sorted by that estimate — Opus as-is, then Opus plan
  first, then Fable — at both the checkbox and the bullet level. A strict dependency that forces another
  order is stated in the entry that carries it.
- Section 1 carries the same **Model:** bullet but is sorted in dependency
  order, each entry naming what it depends on; Astra is named where the plan
  itself is the broad-scope problem.

## Working Rule

1. Start with a self-hosted library or small user-facing acceptance case.
2. Record the current failure with a focused test.
3. Implement the smallest compiler change that makes the program honest.
4. Add positive and negative coverage at the owning compiler phase.
5. Run `scripts/check` before marking the task complete.
