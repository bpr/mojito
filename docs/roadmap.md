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

Every entry carries a stable ID (`R12`). An ID is issued once, by
`scripts/roadmap.py new-id`, and is never renumbered or reused, so cite an
entry anywhere — another entry, a probe, a plan, a commit — by its ID. Work
is grouped into tracks, one per heading below, each named by its `Track:`
slug. An entry's position in its track is its priority, and nothing else.

Each entry's **Depends on** bullet names the IDs it needs, or `nothing`. An
entry that merely makes another one easier counts as a dependency, and an ID
no longer on the roadmap counts as done. The work order is computed, never
maintained by hand: `scripts/roadmap.py list [--track T]` walks the open
entries top-down and puts each one's open prerequisites, from any track,
immediately before it, and `scripts/claude_loop.py` follows that order. So
landing, filing, or moving an entry edits that entry alone. The **Model:**
bullet is an estimate, not a sort key. An entry marked *(standing)* is
guidance kept in view, never scheduled.

Next free ID: **R330**.

## Ordered Work

### Mojo's Pipeline Order: Parametric MIR And One Elaborator

Track: `pmir`.

Mojo checks a parametric body once, keeps it as parametric IR, checks
lifetimes on that IR, and instantiates everything in one elaborator. Mojito
checks most bodies once
([`docs/notes/instantiation-from-template.md`](notes/instantiation-from-template.md)).
An ordinary generic struct's method with no compile-time construct is
instantiated by `native::mono` from its template's MIR. Every other generic
body is still cloned in the AST per instance before the check, so Mojito
instantiates generics two ways: the AST cloner above the waist, and
`native::mono` below it for both backends. The VM's erased dispatch remains
for compile-time evaluation and as the differential oracle. The staged plan that closes the gap is
[`docs/parametric-mir-plan.md`](parametric-mir-plan.md); the stage each entry
belongs to is named in its title.

Scope: only work that moves an instantiation from the AST cloner to the
elaborator, or that deletes what the move makes dead.
A defect found on the way is filed by its kind; a divergence from the pin
goes to a catch-up track, however small.

Frozen: `checker/template_facts.rs` gains no certificate class and no
recipe. A body the certificates do not cover waits for its stage. A
correctness fix to existing behavior is allowed.

- [ ] **R255 (P3b) A collector-less pack-keyed `def` whose signature spreads
  the pack keys a clone**

  Problem: `first_variant[*Ts: Movable]() -> Variant[*Ts]` returning
  `Variant[*Ts](3)` is cloned per explicit application
  (`assets/ok/variadic_pack_forwarding_generic_def.mojo`), and it cannot
  move to its template before R4 and R5 land.
  - The clone is kept by the driver, not by the comptime pack gate:
    `TemplateReach::keyed_methods` (`src/compiler/template_reach.rs`) keys
    the `def` for `Variant[*Ts]`, a struct specialized whole (R4), and for
    `Variant`'s generic `__init__`, a per-call constructor clone (R5).
  - The template's own MIR already types the body as `Variant[param *Ts]`.
  - The caller must close the result: `Checker::infer_generic_call`
    substitutes the result type without `expand_pack_spread`, so a served
    call leaves `Variant[*Ts]` unbound in the caller's slot.
  - The elaborator must expand the spread: `substitute_ty`'s `Ty::Struct`
    arm (`mono/substitute.rs`) would spell `Variant[RuntimePack[..]]`
    instead of the checker's `Variant[(Int, String)]`.
  - The non-pack `wrap` and `wrap_outer` in the same fixture close with R4
    and R5 alone.
  - Depends on R4 and R5.
  - Model: Fable, Planned.

- [ ] **R256 (P3b) A pack spread into a pack-keyed method keys a clone**

  Problem: a `def` that spreads its collector into a method's collector
  (`Sink().take(*a)`) stays on the cloner, since `pack_spread_callees`
  (`comptime.rs`) admits only `print` and a served `def` as the callee.
  - The callee is a method with a type pack of its own, which no template
    serves until R307, so admitting it now would admit nothing.
  - MIR, the elaborator, and the erased oracle already handle the spread
    (`MirInstr::MethodCall::spread`); with R307's exclusion lifted and the
    gate opened, `Sink().tagged(7, *a)` and `Sink().drain(*a^)` ran on the
    VM, erased, and natively with no `def_pack` clone.
  - Admit the callee by method name, judged over every struct method of
    that name: `served_pack_defs` runs before checking, when the receiver's
    type is unknown. A name no struct declares keeps the clone.
  - A spread into a variadic struct's method (`*b: *Self.Ts`) also needs R4.
  - Depends on R307 and R4.
  - Model: Fable, Planned.

- [ ] **R316 (P3b) A pack spread into a constructor keys a clone**

  Problem: `return Tuple[*Ts](*args^)` and `return Tuple(*args^)` in a
  pack-keyed `def` keep the `def` on the cloner, while the pin serves it
  from one template.
  - `pack_spread_callees` (`comptime.rs`) admits only `print` and a served
    `def` as a spread's callee.
  - `Tuple` and `Variant` are variadic structs specialized whole, so a
    served body has no instance to name (R4).
  - Reached by `conformance/fixtures/empty_runtime_pack.mojo`,
    `tuple_values.mojo`, `pack_scope_restore.mojo`, and
    `nested_heterogeneous_packs.mojo`, all pin-accepted.
  - A spread into a plain struct's own-pack `__init__` is a method with a
    pack of its own, so it can land with R256's admission by name.
  - Depends on R4.
  - Model: Fable, Planned.

- [ ] **R253 (P3b) The cloner's type-pack branch is still live**

  Problem: `pack_generic_template_names`, the `pack_generics` arm of the
  explicit-application dispatch (`comptime/mono.rs`), and the
  `unspecialized type-pack function` stub (`comptime/specialize.rs`) still
  clone the pack-keyed `def`s the template does not serve.
  - They are reached by a spread into a method (R256), a spread into a
    constructor (R316), and a `comptime for` over a list in a pack-keyed
    `def` (R246).
  - Delete the branch, `served_pack_defs` and its gate, and the
    `Elab::pack_generics`/`served_packs` fields once nothing reaches them;
    `--instantiation-census` over the pack fixtures must show `def_pack 0`.
  - Keep `def_pack_names`, `generate_spec`'s `type_pack_expansions` (R4,
    R6), and `DefSpecializationRequest::with_variadic`.
  - Depends on R256, R316, and R246.
  - Model: Fable, Planned.

- [ ] **R246 (P3b) A `comptime for` over a named collection, a display
  over a parameter, a value pack, or a reflection query, or with a local
  `comptime` binding in its body, still keys a clone**

  Problem: the template serves a `comptime for` only over a `range` of
  parameter expressions or a list, set, or dictionary display of literals,
  with no `comptime` binding in its body (`comptime_for_is_template_served`);
  every other loop of a generic `def` is unrolled in the AST on a clone per
  instantiation.
  - A module collection constant (`comptime L = [10, 20]`, then `comptime
    for x in L` in `def f[n: Int]()`) fails on the clone path with "cannot
    materialize comptime value of type 'Array[Int, Int(2)]'", where the pin
    prints `11`, `21`; serving it needs the cloner's predicate to know the
    module's constants, as `Elab::top_consts` does.
  - A display over a binder (`[n, n + 1]`) needs the elements as a thunk
    the elaborator runs (`ComptimeThunks` with a non-`Bool` result); one
    over an enclosing loop's variable also needs R248.
  - A variadic value pack (`comptime for v in vals`, `*vals: Int`) waits on
    its `def` being served at all (R318).
  - A body binding (`comptime j = i * 2`) is R290's, and a reflection bound
    or sequence (`range(reflect[T].field_count())`, `field_names()`) is
    R267's, with R290 for the bound `comptime r = reflect[T]` form
    (`assets/ok/reflection_symbolic_fields.mojo`,
    `assets/ok/reflection_field_name_materialize.mojo`).
  - The header already iterates a sequence (`ComptimeSequence::Elements`,
    schema 1.25), so each shape needs only its sequence compiled.
  - Depends on R290, R267, and R318.
  - Model: Fable, Planned.

- [ ] **R257 (P3c) A lane gather at a symbolic width keys a clone**

  Problem: `v.shuffle[...]()`, `v.slice[w, offset=o]()`, and `v.join(w)` in
  a `DType`- or width-keyed `def` keep the `def` on the cloner
  (`lane_def_shape_served`), where every other lane form is served by the
  template.
  - `SimdShuffle` carries its mask as a list of known lane indices, which
    `join` and `slice` build from the receiver's width; the checker records
    the adjustment only at a known width (`infer_simd_join`,
    `infer_simd_slice`, `infer_simd_shuffle`).
  - The gather needs a form over the width's parameter expression — a
    `join` as "the receiver's lanes then the argument's", a `slice` as an
    offset and an output width — that the elaborator expands to the mask
    once the width is known.
  - `assets/ok/simd_symbolic_surface.mojo`'s `doubled` is the body that
    waits.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R258 (P3c) A `DType` float query over a binder keys a clone**

  Problem: `DType.mantissa_width[dt]()` and the other float-format queries
  over a `DType` binder keep a `def` on the cloner, where the checker folds
  them only at a known dtype (`DtypeFloatQuery`).
  - The answer is a function of the dtype, so it is a parameter expression
    the elaborator can evaluate once `dt` is bound, as the defaulted
    `to_bits` target already is (`symbolic_unsigned_dtype_of`).
  - `assets/ok/dtype_float_queries.mojo`'s `mantissa_of` is the body that
    waits.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R259 (P3c) A hash of a lane value in a lane-keyed `def` keys a
  clone**

  Problem: `hash(v)` over a vector whose dtype or width is a binder keeps
  the `def` on the cloner: the hasher's SIMD leaf is demanded by the closed
  vector type (`record_hash_leaf` skips a symbolic one), and a served
  template has none to demand.
  - The elaborator must demand the leaf for each instance's closed vector
    type, as it enqueues a display instance for `print`.
  - A `hash` of a width-one lane (`hash(v[0])`) needs no leaf and is served
    today; the exclusion is syntactic and keeps every `hash(...)` call.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R261 (P3c) A local `comptime` binding in a lane-keyed `def` keys a
  clone**

  Problem: `comptime lane = dt` in a `DType`-keyed `def` keeps the `def` on
  the cloner (`lane_def_shape_served`): the cloner's body elaboration
  evaluates a local `comptime` binding before the check, and a value over
  the `def`'s own binder is "not a compile-time type" there.
  - A served template needs the binding left to the checker, which already
    binds a local `comptime` `DType` symbolically (`comptime_dtypes`), and
    MIR lowers the statement as an ordinary binding of its value.
  - The same gap keeps a `comptime for` with such a binding on the cloner
    (R246).
  - `assets/ok/simd_symbolic_surface.mojo`'s `rebound` is the body that
    waits.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R260 (P3c) An overloaded lane-keyed `def` is cloned per call**

  Problem: a `DType`- or width-keyed `def` sharing its name with another
  declaration (`kind[dt: DType](a: Scalar[dt])` beside `kind(a: String)`)
  stays a template family on the cloner (`dtype_generic_template_names`),
  while a uniquely named one is served by its template.
  - A bound generic is admitted by name (`collect_bound_generic_templates`
    requires a unique name), since overload selection is the checker's and
    the elaborator's registry is name-keyed; the checker's recorded
    instantiation names the selected overload, which is what a served call
    would carry.
  - The `DTypeVectorDef` census class and the `DType` stub path
    (`dtype_generic_template_names`, `omits_dtype_param`) go when this and
    R257–R259 land; a lane-keyed struct member is R4's and a method's own
    lane binder R5's.
  - `assets/ok/dtype_keyed_overload_family.mojo` and
    `assets/ok/dtype_keyed_overload_beside_plain.mojo` are the bodies that
    wait.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R312 (P3) A generic struct's method that binds a local `comptime`
  over its struct's parameters still clones per instance**

  Problem: `comptime k = Self.n + 1` or `comptime U = Self.T` in a generic
  struct's method, and a `comptime for` the template does not serve (over a
  compile-time list or a reflection query), still stub the template and
  mint a clone per instance, where a `comptime if` or a `range` loop over
  `Self.T` or `Self.n` is served by the template.
  - The elaborator walks the method with the struct's parameters open as
    binders (`Elab::def_body`, `comptime/elab.rs`), but evaluates a local
    `comptime` binding before the check, where the binder has no value; the
    body then falls back to `unspecialized_method_stub`, and
    `keyed_methods` (`comptime/specialize.rs`) clones it.
  - This is the method case of R290 (a local `comptime` over a `def`'s
    binder) and of R246 (an unserved `comptime for`); the fix is theirs.
  - The census class `InstanceMethodComptime` counts what is left.
  - Depends on R290 and R246.
  - Model: Opus, Not Planned.

- [ ] **R328 (P3) A thunk condition cannot read a local `comptime` binding
  over a binder**

  Problem: `comptime m = n + 1` then `comptime if is_even(m):` in `def
  k[n: Int]()` fails with "VM CTFE failed for `$comptime$k$0$mono$V1`:
  operator Mod is not defined for None and Int", where the pin prints `k`.
  - The checker binds `m` to its parameter expression, so a compiled
    condition over it closes, but the thunk lifted for an application reads
    `m` as a frame slot no binder fills.
  - The thunk should read `m` as the parameter expression it denotes
    (`Const::Param`), as it reads an enclosing `comptime for` index.
  - Found while landing R248 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R4 (P3d) A vector-keyed, struct-valued, or variadic struct is
  specialized whole in the AST**

  Problem: `Tuple`, `TString`, `Variant`, a user variadic struct,
  `AHasher[key: U256]`, and a struct keyed on a struct-typed value are
  re-declared per instance by the cloner, from requests the driver derives
  and applications the elaborator finds syntactically.
  - A struct keyed on a `DType` or a lane width is already a generator
    (2026-10-05); these are what remain.
  - Their members need a method's own compile-time parameters (R5):
    `Tuple.__getitem_param__[idx]`, `__contains__[T]`, `Variant.isa[T]`,
    and `AHasher._update_with_simd(SIMD[_, _])`.
  - Nearly every `Tuple` member holds a `comptime for` (R305).
  - `native::mono` must bind a struct's pack: `bind_ty_args`
    (`mono/unify.rs`) has no pack case, and `substitute_ty` does not expand
    a spread inside a field's `__RuntimeTuple[*Self.Ts]`.
  - The erased oracle and CTFE's AST route must run a variadic generator,
    with the pack's elements reified on the instance.
  - `Tuple`'s synthesized `reverse` and `concat` and its value twins
    (`__getitem_param_value__$k`) have no upstream counterpart: the pin
    writes the transforms in source over type-list operations.
  - The `Tuple` and `TString` request types, `variadic_struct_requests`,
    and `pending_struct_instances` leave the driver and elaborator with
    them.
  - Depends on R5 and R305.
  - Model: Fable, Planned.

- [ ] **R308 (P3e) The hasher's `SIMD[_, _]` leaf is minted for every hasher
  and cloned per call**

  Problem: `Fnv1a._update_with_simd` and `AHasher`'s are cloned for fourteen
  eager leaf types in every program (`eager_hash_leaf_types`,
  `hasher_leaf_requests`), and per demanded vector, where the pin's
  `_update_with_simd(mut self, value: SIMD[_, _])` is one generator with two
  implicit parameters.
  - The desugar binds one `$simd: $SIMD` type parameter whose template body
    never checks, so the body is a trap stub; upstream's shape is a `DType`
    and a width binder over `SIMD[dt, w]`, which a template serves once both
    are inferred from the argument (R309).
  - The scalar `__hash__` leaf dispatch names the clone
    (`symbol::simd_update_clone_name`) in `native::mono`, the VM, and
    Pliron (`lower/print.rs`, `lib.rs`, `lower/methods.rs`); each must name
    the template's instance instead.
  - CTFE mints its own leaves (`ctfe.rs`), and `AHasher`'s are members of a
    struct specialized whole (R4).
  - Depends on R309 and R4.
  - Model: Fable, Planned.

- [ ] **R310 (P3e) A method whose body only a per-call clone can serve still
  clones per call**

  Problem: a method with compile-time parameters of its own keeps its
  per-call clones where its body reaches a compile-time-keyed stub, holds a
  nested `def` or a lambda, or applies a tuple or a struct specialized whole
  over its own binders, so `per_call_method_clones` and the clone-name
  retargeting it feeds are still live.
  - The elaborator decides a stub-reaching method after a round's walk and
    reports it (`Elaborated::stub_reaching_methods`); the driver keys it for
    the next round, beside what `TemplateReach::method_call`
    (`src/compiler/template_reach.rs`) reads off the checked body at each
    closed call.
  - A member of a struct specialized whole mints its own per-call clones
    (`generate_struct_spec`, `generate_value_struct_spec`); a bundled
    variadic member's body uses the checker's `Variant` operations, which
    need `T` closed, and a user one mixes a folded pack loop with a
    `comptime if` over its own binder (`assets/ok/variadic_method_type_params.mojo`).
  - When the last class goes, delete `per_call_method_clones`, `PerCallBase`,
    the driver's `method_specialization_requests`, the checker's
    `specialized_method_clone`, `instance_call_method_clone`,
    `clone_serves_overload`, and `per_call_constructor_target`,
    `MethodProvenance::PerCallConstructor`, and the per-call clauses of the
    `MethodFeatures` certificate bits.
  - Depends on R4, R6, R7, R307, and R308.
  - Model: Fable, Planned.

- [ ] **R6 (P3e) A nested `def` over an enclosing compile-time parameter is
  cloned**

  Problem: a nested `def` that reads its enclosing function's compile-time
  parameter, or captures across one, is re-minted with each clone of the
  enclosing body.
  - The nested body is a generator that names the enclosing binders, and its
    captures are part of its contract.
  - Delete the cloner's nested branch and the certificate class.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R288 (P3e) A nested `def` that names its enclosing `def`'s type
  binder fails MIR verification**

  Problem: `def inner(y: T)` inside `def outer[T: Writable & Copyable](x:
  T)`, and `return Us.length` in a nested `def` over the enclosing pack,
  fail with "MIR function 'outer$inner' … names parameter `T` of `outer`
  that no enclosing declaration binds", while the pin runs both.
  - The cloned `outer` runs; the failure is the template's own MIR for the
    nested function, whose scope (`Scope::of_function`, `verify/scope.rs`)
    declares its own binders and its struct's but not its enclosing
    function's.
  - The nested body is a generator under the enclosing binders, as R6
    states; MIR needs to record which declaration encloses it.
  - A value binder works, because the nested body reads it as a captured
    slot.
  - Found while landing R62 (2026-10-04).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R317 (P3e) A nested generic `def` whose binder no runtime
  parameter names fails to elaborate**

  Problem: `def inner[U: AnyType](y: Int) -> Int` nested in a non-generic
  `outer`, called as `inner[Int](3)`, fails with "unsupported
  monomorphization cannot resolve parameter `U`", while the pin prints 3.
  - The same `def` at top level runs, so the gap is the nested cloner's.
  - Unlike R288 the binder is the nested `def`'s own, not its enclosing
    function's.
  - Found while landing part of R253 (2026-10-05).
  - Depends on R6.
  - Model: Opus, Not Planned.

- [ ] **R329 (P3e) A nested `def` inside a `comptime for` cannot read the
  index**

  Problem: `def inner(): print("in", i)` in a `comptime for i in range(n)`
  body fails with "Undefined variable 'i'", in a generic `def` and in a
  plain `main` alike, where the pin prints `in 0` and `in 1`.
  - The index is a compile-time binder of the loop, which the nested body
    neither captures nor declares.
  - Unlike R110 the value is a loop index rather than a local `comptime`
    binding, and unlike R6 and R288 its binder is the loop's, not the
    enclosing `def`'s.
  - Found while landing R248 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R7 (P3e) A compile-time evaluation mints its own clones**

  Problem: a generic call inside a compile-time evaluation is cloned in the
  AST subprogram that evaluation builds, outside every other instantiation
  path.
  - The evaluation requests the instance from the worklist, by the request
    path already designed.
  - The path's first landing (2026-10-03, `Specializer::demand_application`)
    materializes every pending instance before a compile-time call and
    verifies the whole completed output as the fragment the VM runs; the
    reference closure of the thunk alone is the refinement, and the effect
    scan follows `Call` edges only.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R8 (P3) A method that reaches a compile-time construct still
  clones per instance**

  Problem: a generic struct's method keeps its per-instantiation clone where
  its body holds or reaches a construct MIR cannot express yet, so
  clone-symbol retargeting and the method certificate classes are still live.
  - Decided from syntax by the cloner (`keyed_methods`,
    `comptime/specialize.rs`): a `comptime if` or `comptime for`, a `rebind`,
    a nested `def` or a lambda, and a call that reaches a
    compile-time-keyed `def`.
  - Read from the template's checked types by the driver
    (`src/compiler/template_reach.rs`): a tuple or a struct specialized whole
    over the struct's parameters, and a call of an overloaded method with
    binders of its own.
  - Each class goes with the entry that gives it a MIR form.
  - The erased oracle cannot follow a template-served body's call to an
    instance's clone, because an erased value carries no type arguments:
    `--erased` stops with "Box.kind: unspecialized type-keyed method" where
    concrete MIR names the clone from the substituted receiver type.
    `assets/ok/generic_struct_template_reach.mojo` is the `ERASED_VM_RESIDUE`
    row (`tests/corpus_test.rs`), and it goes when these clones do. The
    stage-composed `Backend::run` seam and CTFE run erased bodies too.
  - When the last goes, delete `generate_instance_clones`, the driver's
    keyed-method and template-reach plumbing, the clone-symbol retargeting in
    the checker, the VM, and `native::mono`, and the method certificate
    classes in `checker/template_facts`.
  - Depends on R1, R4, R5, and R6.
  - Model: Fable, Planned.

- [ ] **R314 (P3) A rebound place is spelled by its terminal type rather
  than an explicit step**

  Problem: MIR reads or writes the operand place of `rebind[Dest](place)`
  through a place whose terminal type is `Dest` while its projections keep
  the storage's own type, where upstream's reference overload rebinds the
  pointer it returns with an explicit `kgen.rebind`.
  - Only a rebind produces such a place, and `native::mono` judges every
    place whose two types disagree as one (`mono/rebind.rs`), but nothing in
    the place itself says it was rebound.
  - A `Proj::Rebind` step would say it; every ownership pass and the VM
    test a place for no projections to mean its whole root, so the step
    must read as transparent there first.
  - The value form is already explicit (`MirInstr::Rebind`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R9 (P4) The driver elaborates and checks to a fixpoint**

  Problem: `compile_linked` re-elaborates and re-checks for up to five
  discovery rounds, because only a check discovers the instances the next
  elaboration must clone.
  - With no clone left, the check runs once on the linked source, and source
    validation and the executable check are one pass.
  - The elaborator's worklist finds instances transitively from the entries.
  - CTFE's AST route is deleted, `VmBackend::run_function_value` and the
    per-round fuel reset with it. The worklist's request path
    (`docs/notes/ctfe-request-path.md`) is the only one.
  - Module-scope `comptime` values follow decision D3: a constant the
    `ParamExpr` folder closes is folded before the check, and one whose
    initializer applies anything is a typed request the elaborator
    evaluates on first demand. The early folding of applied constants goes
    here.
  - The budget is plan decision D4: a stated improvement on the workloads
    repeated checking dominates, and bounded regressions elsewhere.
  - Depends on R8.
  - Model: Fable, Planned.

- [ ] **R10 (P5) The replaced mechanisms are still in the tree**

  Problem: once R9 lands, the AST cloner's core, template derivation, and
  the VM's erased dispatch serve nothing.
  - Delete `comptime/{rewrite,specialize,mono,nested}.rs` down to what CTFE
    and module-scope folding need.
  - Delete what is left of `checker/template_facts` and the derivation
    vocabulary in `mojito-checked/src/templates.rs`.
  - Delete erased dispatch from the VM and its tolerances from `mir::verify`.
  - Delete the oracle with it: `Backend::run_elaborated`, `VmInstantiation`,
    `--erased`, `MOJITO_VM_ERASED`, and the corpus binary's `erased_vm` group.
  - The stage-composed `Backend::run(&CheckedProgram)` seam runs erased
    bodies, because `mojito-vm` cannot call the elaborator. Move the tests
    that use it to the driver, or give the seam concrete MIR.
  - Make a last pass over `docs/architecture.md` and `AGENTS.md` invariant 3.
    Each earlier stage updated the pipeline it changed.
  - Depends on R9.
  - Model: Fable, Planned.

- [ ] **R11 (P6) The standard library is checked again in every
  compilation**

  Problem: Mojo imports a package without checking its source again, and
  Mojito re-parses and re-checks the bundled library each time.
  - Serialize verified, drop-elaborated generators as a package artifact.
    That boundary is Mojito's choice. Upstream's `.mojoc` holds the
    post-parse IR, written before lowering.
  - A package keeps every exported template, reached by an entry or not.
  - Importing skips the source check. It still validates the artifact and
    checks each instance's obligations.
  - The bundled library is the first consumer, built once per compiler build.
  - Depends on R10.
  - Model: Fable, Planned.

- [ ] **R12 (P5) The erased oracle cannot default-construct a SIMD-typed
  parameter**

  Problem: `Array[c_char, 4]()` runs on concrete MIR and stops under
  `--erased` with "constructing type parameter 'T' ... requires a reified
  type argument".
  - `T()` over a parameter bound to a SIMD type is the zero vector. The
    elaborator builds it (`mono/substitute.rs`,
    `default_construct_simd_parameters`).
  - An erased value carries no type argument for a native scalar or vector,
    so the VM's `ConstructTypeParam` has nothing to construct from.
  - `assets/ok/simd_parameter_default_construction.mojo` is the
    `ERASED_VM_RESIDUE` row (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and this row with it. Nothing else needs
    the erased path to construct one.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R13 (P5) The erased oracle reads a place pointer bound to a
  parameter as its pointee**

  Problem: `assets/ok/template_served_loan_carrying_instance.mojo` runs on
  concrete MIR and stops under `--erased` with "checked nominal subscript
  receiver is Int".
  - `assets/ok/template_served_def_loan_carrying_argument.mojo` and
    `assets/extensions/ok/template_served_iterable_def.mojo` stop the same
    way, in a `def` its template serves.
  - A `Pointer(to=x)` value is a reference handle on the VM. A concrete body
    tells it from a reference by the `Pointer` type of the register or the
    slot.
  - An erased body types it `T`, so `value.copy()` and a read of the whole
    slot chase the handle to the pointee.
  - Each fixture is an `ERASED_VM_RESIDUE` row (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and these rows with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R14 (P5) The erased oracle spells a type name over a parameter
  as written**

  Problem: `_unqualified_type_name[Self]()` in a method of `Box[T]` prints
  `Box[SIMD[DType.int, 1]]` on concrete MIR and `Box[T]` under `--erased`.
  - The template carries the type in `MirInstr::TypeName`, and the elaborator
    writes the name from the substituted type.
  - An erased value carries no type arguments, so the oracle has nothing to
    substitute and spells the template's parameters.
  - `repr` of a `List`, `Dict`, `Set`, `Array`, or `Optional`, and the text
    of `EmptyOptionalError`, differ the same way.
  - `assets/ok/generic_struct_instance_bodies.mojo`,
    `generic_struct_instance_dispatch.mojo`,
    `optional_raising_subscript.mojo`, and
    `template_method_string_builtins.mojo` are the `ERASED_VM_RESIDUE` rows
    (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and these rows with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R15 (P5) The erased oracle cannot size a parameter or dispatch a
  bound requirement to a witness that renames its binder**

  Problem: `size_of[T]()` in a `def` its template serves runs on concrete
  MIR and stops under `--erased` with "a symbolic type has no layout: T",
  and `self.item.__hash__(hasher)` through `T: Hashable` on a struct
  overloading `__hash__[H: Hasher]` stops with "unknown method
  'Twin.__hash__'".
  - An erased body has no type for `T`: the elaborator answers the
    instance's `MirInstr::SizeOf` under its target, and the oracle runs the
    template's on the host.
  - A bound dispatch spells the requirement's qualifier
    (`__hash__$ov$Some$u5B$Hasher$u5D$$Hasher`), which names no lowered
    overload of the receiver when the witness binds the parameter under a
    name of its own (`Twin.__hash__$ov$H$Hasher`); the elaborator matches
    them by parameter types (`dispatched_overload_target`), the VM's
    by-name dispatch does not.
  - The oracle reifies a solved type argument's struct name from the call
    (`MirInstr::Call::instantiated_args`) but cannot construct a SIMD alias
    (`Float32`) or a struct over value parameters (`Array[Int, 2]`) from it.
  - `assets/ok/size_of_builtin.mojo`, `template_served_def_closed_call.mojo`,
    `overloaded_method_own_binder_symbols.mojo`,
    `simd_nullary_construction.mojo`, and `tuple_array_defaultable.mojo` are
    the `ERASED_VM_RESIDUE` rows (`tests/corpus_test.rs`).
  - An erased frame decides a `comptime if` over a value binder from its
    reified parameters (`comptime_branch_holds`), and one comparing a type
    binder with a type its reified spelling names (`T == Int`), but no other
    type predicate, and runs no thunk for a condition that applies a
    function: the remaining type-keyed `comptime_if_*` fixtures and
    `comptime_if_condition_applies_def.mojo` are rows too. A region inside
    a `try` reads only binders its body also holds as locals.
  - Entry R10 deletes the oracle and these rows with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R16 (P5) The erased oracle cannot run a default that reads a
  compile-time parameter**

  Problem: `V[3]().m()`, beside `def m(self, x: Int = Self.n * 2)` in
  `struct V[n: Int]`, prints `6` on concrete MIR and stops under `--erased`
  with "erased default for parameter 'x' of 'V.m' reads a compile-time
  parameter".
  - The default function declares the binders it reads, and the elaborator
    instantiates it with the owner instance's arguments
    (`default_function_bindings` in `mono/specializer.rs`).
  - The VM's `bind_for_call` runs an erased default function with no
    frame values, so it refuses one that declares binders rather than read
    them as `None`.
  - `assets/ok/default_reads_binder.mojo` is the `ERASED_VM_RESIDUE` row
    (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and this row with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R249 (P5) The erased oracle resumes a kept `comptime for` mid-range
  after a compile-time `break`**

  Problem: the erased path runs a `ComptimeFor` header from its index slot
  — nothing before the first iteration, the previous value after — and
  clears the slot only when the range ends (`comptime_for_next`,
  `backend/vm.rs`), so a frame that leaves the loop through a `break` and
  enters it again, from a runtime loop around it, resumes at the index the
  `break` left.
  - Concrete MIR carries no header: the elaborator unrolls the loop, so the
    production path is unaffected.
  - A per-frame loop state, or a slot reset on every exit edge, fixes it;
    entry R10 deletes the oracle and the question with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R292 (P5) The erased oracle cannot run a vector at a kept `comptime
  for` index's width**

  Problem: `var v = SIMD[DType.int32, i](7)` inside `comptime for i in
  range(1, n)` in a generic `def` runs on concrete MIR and stops under
  `--erased` with "a SIMD instruction over the symbolic slots
  `SIMD[DType.int32, i]` reached the VM".
  - Concrete MIR has no such instruction: the elaborator unrolls the loop
    and gives each copy its own slot at its own width.
  - The erased frame runs the kept header with the index as a runtime
    value, and the VM's SIMD instructions need a known width
    (`concrete_simd_slots`).
  - `assets/ok/comptime_for_index_typed_local.mojo` is the
    `ERASED_VM_RESIDUE` row (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and this row with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R17 (P5) The erased oracle cannot bind a static receiver from a
  binder its frame does not hold**

  Problem: `W[n].plus(1, 2)` in `def via[n: Int]()`, beside a static
  `plus` reading `Self.k` of `struct W[k: Int]`, prints `12` on concrete
  MIR and stops under `--erased` with "no field 'k'".
  - The VM binds a static method's receiver-less `self` slot from the
    call's spelled receiver (`static_receiver_binding` in `mojito-vm`),
    resolving a binder through the caller's frame.
  - An erased `via` keeps no slot for an `n` its body never reads as a
    value, so the receiver's argument has nothing to resolve to.
  - No fixture pins it; `assets/ok/static_method_reads_struct_value_parameter.mojo`
    keeps to receivers the erased frame holds.
  - Entry R10 deletes the oracle and this gap with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R286 (P5) The erased oracle loses a pack's length once its
  collector is gone**

  Problem: `return 2 + Us.length` over `var *extra: *Us` that the body never
  reads, or over a pack spelled only in the call
  (`count[Int, String, Bool]()`), prints on concrete MIR and stops under
  `--erased` with "the erased oracle cannot evaluate the parameter constant".
  - The oracle reads a pack's length from its collector's runtime arity
    (`erased_parameter_values`, `backend/vm.rs`); drop elaboration destroys
    an unread `var` collector at entry, and a collector-less signature has
    none.
  - A `comptime for i in range(Us.length)` stops the same way.
  - `assets/ok/pack_length_runtime_position.mojo` is the
    `ERASED_VM_RESIDUE` row (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and this row with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R287 (P5) The erased oracle cannot answer a pack's membership or
  conformance**

  Problem: `Us.contains[Int]()` and `Us.all_conforms_to[Writable]()` in a
  runtime position print on concrete MIR and stop under `--erased`.
  - The erased frame carries one placeholder element per pack argument, so
    it answers the length and nothing that needs an element's type.
  - The elaborator answers both through the oracle that decides a
    `comptime if` (`answer_param_constants`, `mono/specializer.rs`).
  - Entry R10 deletes the oracle and the question with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R323 (P5) The erased oracle cannot answer a reflection query over a
  type parameter**

  Problem: `return reflect[T].field_count()` in a template-served
  `def count[T: AnyType]()`, and the same query in a generic struct's method
  or a `comptime if` condition, print on concrete MIR and stop under
  `--erased` with "the erased oracle cannot evaluate the parameter constant".
  - The query reaches MIR as a parameter constant over `T`, which the
    elaborator answers per instance (`reflection_answer`,
    `mono/symbolic.rs`); an erased frame carries no type argument to answer
    it from.
  - `assets/ok/reflection_template_served.mojo` is the `ERASED_VM_RESIDUE`
    row (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and this row with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R295 (P5) The erased oracle cannot construct a pack element**

  Problem: `print(Ts[0]())` in `def ends[*Ts: Movable & Defaultable &
  Writable & Deinitable]()` prints on concrete MIR and stops under
  `--erased` with "constructing type parameter 'Ts' … requires a reified
  type argument".
  - The VM indexes a pack's reified spellings by the element's index
    register, but the erased frame of an explicit application leaves the
    pack's slot unbound.
  - A pack proved `Defaultable` only by a `where` clause has no slot at
    all, since `constructible_type_parameter` reads bounds.
  - Under a `comptime for` the oracle stops first at the loop's bound
    (R286).
  - `assets/ok/pack_element_default_construction.mojo` and
    `assets/ok/pack_element_binding_served.mojo` are its
    `ERASED_VM_RESIDUE` rows (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and these rows with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R300 (P5) The erased oracle cannot run a vector whose width is a
  layout query**

  Problem: `SIMD[DType.float32, S]` for `comptime S = size_of[Pair]()`
  runs on concrete MIR and stops under `--erased` with "a SIMD instruction
  over the symbolic slots `SIMD[DType.float32, size_of[Pair]()]` reached
  the VM".
  - The elaborator answers the query under the compilation's target; the
    erased frame closes a lane only from the binders it reifies, and a
    layout query names none.
  - `assets/ok/comptime_layout_constant.mojo` is its `ERASED_VM_RESIDUE`
    row (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and the row with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R301 (P5) The erased oracle cannot stop a kept `comptime for`
  bounded by a binder**

  Problem: `comptime for i in range(n)` in a template-served `def` keyed on
  `n`, or over `range(len(Ts))`, runs on concrete MIR and stops under
  `--erased` with "the erased oracle cannot decide the comptime for stop".
  - The elaborator unrolls the loop per instance; the erased frame has no
    runtime value for the bound.
  - `assets/ok/comptime_for_template_served.mojo` and
    `assets/ok/pack_element_alias_served.mojo` are its
    `ERASED_VM_RESIDUE` rows (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and these rows with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R311 (P5) The erased oracle cannot forward a type binder to a
  template's own call**

  Problem: a template-served body that calls a generic callee over its own
  type binder (`return 1 + S.count(x, n - 1)` in `S.count[T]`, or a `def`'s
  recursive call) runs on concrete MIR and stops under `--erased` with "the
  erased oracle cannot decide the comptime if condition" in the callee.
  - The VM reifies a call's solved type argument only when it is closed
    (`supplied_parameter_arguments`, `backend/vm.rs`); one over the
    caller's binder reads the caller's frame (`bound_type_parameter`),
    which holds no slot for a binder its body never reads as a value.
  - A `comptime if` comparing a method's binder with its struct's
    (`U == Self.T`) stops the same way: an erased instance carries no type
    argument.
  - `assets/ok/comptime_if_inferred_static_method.mojo` and
    `assets/ok/generic_method_per_call_clones.mojo` are its
    `ERASED_VM_RESIDUE` rows (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and these rows with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R315 (P5) The erased oracle cannot decide a conformance condition
  over a type binder**

  Problem: `comptime if conforms_to(Self.T, Hashable)` in a template-served
  method runs on concrete MIR and stops under `--erased` with "the erased
  oracle cannot decide the comptime if condition `Conforms { … }`".
  - An erased frame binds a struct's type parameter to the spelling of its
    type (`Box[Int]` binds `T` to `Int`), which names no conformance table.
  - `assets/ok/keyed_def_builds_loan_carrying_instance.mojo` is its
    `ERASED_VM_RESIDUE` row (`tests/corpus_test.rs`).
  - Entry R10 deletes the oracle and the row with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Native Backend

Track: `native`.

The ABI-bump collector is last whatever else moves, because it batches every
change that needs a new `MJRT_ABI_VERSION`.

- [ ] **R277 Native `Tuple.reverse` and `Tuple.concat` over a `String`
  element double-free it**

  Problem: `var t = (1, String("x"))` then `t.reverse()` (or
  `t.concat(Tuple(True))`) prints the result and then traps with "vm: double
  free of Pointer allocation" natively, where the pin and the VM run to
  completion.
  - An `Int`/`Bool` tuple is clean; the `String` element is destroyed both in
    the result and in the receiver the transform read.
  - The transform result is built by the `TupleTransformRequest` clone, so
    the suspect is the native lowering copying the element without the
    `String` copy that the VM's clone performs.
  - Pinned by `conformance/probes/native_tuple_transform_string_element.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R299 Native lowering cannot lay out a struct a served pack binds
  only as an element**

  Problem: `show(String("tmp"), [7, 8])` and `show(Named("k", w), …)` into
  a template-served `def show[*Ts: Writable](*args: *Ts)` stop natively with
  "unsupported aggregate layout (struct 'List' has no MIR declaration to lay
  out)", where the VM and the pin print
  (`assets/ok/pack_argument_destroyed_after_call.mojo`,
  `assets/ok/pack_element_temporaries.mojo`, both `exe-differential` rows
  of `conformance/pliron-parity.tsv`).
  - The struct appears only as an element of the call's pack, so no
    declaration of it reaches the native program's layout table.
  - Before R252 landed, `pack_element_temporaries` stopped earlier, at
    "unsupported heterogeneous runtime pack projection": a folded constant
    index into a served `def`'s pack was projected by stride.
  - Found while landing R252 (2026-10-04).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R262 Finding a nested instance's method clone still spells every
  level of its type**

  Problem: when the checker minted a per-instantiation clone of a
  template's method, the elaborator names the clone an instance needs by
  respelling the instance's whole argument type, so a deeply nested
  instance of that template pays its depth on each lookup.
  - `template_spelled_arguments` rebuilds each level under its template
    name, and `mangle` renders the whole type for the clone's symbol.
  - A template with no minted clone of the method skips both
    (`instance_method_clone`), so the nesting probes are linear.
  - The lever is a clone index keyed by the instance's arguments rather
    than by a spelled symbol.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R275 Native `repr` of a `SIMD` vector wider than one lane is
  unsupported**

  Problem: `repr(SIMD[DType.int32, 2](1, 2))` prints
  `SIMD[DType.int32, 2](1, 2)` at the pin and on the VM, but the native
  backend stops with "unsupported type `Simd { … width: Known(2) }`".
  - Pliron's `lower_repr_builtin` (`lower/methods.rs`) labels only scalar
    registers; a vector register reaches no arm.
  - The text is upstream's `SIMD.write_repr_to`: `SIMD[`, the dtype's repr,
    the width, then the lanes as `print_simd` writes them.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R18 Native runtime ABI bump: land every change that needs a new
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
  - The signature classification changes the native calling convention itself,
    and the ABI version, `docs/native-abi.md`, the runtime, and the parity
    harness all move together.
  - Depends on nothing. Every later change that needs an ABI bump joins this
    entry instead of waiting on it.
  - Model: Fable, Planned.

### Catch Up To Current Mojo: Ownership, Origins, And Destruction

Track: `ownership`.

Every catch-up track closes a gap between Mojito and the pinned Mojo. Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

- [ ] **R20 A value live after a region is never destroyed when the region
  raises out of the function**

  Problem: `var b = Thing(); if cond: raise Error("boom"); look(b)` never
  runs `b`'s destructor when the raise propagates, where the pin destroys
  `b` on the raise edge.
  - The same leak follows a raising call with the value live after it, a
    raise inside a loop, a raise in an `else` arm, and a raise inside a
    `comptime if` arm; only a value dead at the raise is destroyed, since
    its drop precedes the raise.
  - A `try` in the same function cleans up its body's locals on the
    exceptional edge; a raise that leaves the function has no cleanup list.
  - Decide where the cleanup lives: a per-function unwind list the VM and
    the native runtime both run, or drops elaborated on the raise edge.
  - Probe: `conformance/probes/raise_path_live_value_leaks.mojo`
    (`docs/notes/comptime-region-ownership.md`, `c7`).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R30 A `var` of an opaque type is not required to be `Deinitable`**

  Problem: `var v: FT = FT()` under `comptime if conforms_to(FT, Defaultable
  & Writable):` validates at Mojito, while the pin reports "'v' abandoned
  without being explicitly destroyed ... consider adding trait conformance
  to Deinitable" until `Deinitable` is proved too.
  - Mojito accepts a program the pin rejects, so this is a divergence; it
    holds for every opaque parameter, not only a reflected field type.
  - The abstract destruction walk (`explicit_destroy.rs`) does not ask the
    view's bounds for `Deinitable`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R31 An indexed element that is implicitly copyable may be
  transferred**

  Problem: `return x[0]^` over a `List[P]` with an `ImplicitlyCopyable`
  struct `P` runs on Mojito and copies the element, where the pin reports
  "expression does not designate a value with an origin".
  - The pin admits the transfer only for a trivial register value such as
    `Int`, and warns that it has no effect.
  - Mojito's `Transfer` inference (`checker/inference.rs`) admits every
    implicitly copyable indexed element.
  - Narrowing the test to trivial register values is the lever.
  - Probe: `conformance/probes/indexed_copyable_element_transfer.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R32 Owned iteration of a variadic pack moves elements the pin copies**

  Problem: `for var value in values^` over `var *values: Self.T` moves each
  element out, so Mojito runs it for a `String` element; the pin iterates
  the pack by copy and rejects it ("value of type 'T' cannot be implicitly
  copied").
  - Over an implicitly copyable element (`var *values: Int`) both run.
  - The bundled `List`, `Set`, and `Array` literal initializers use this
    spelling; upstream's consume the pack with `consume_elements`.
  - Pinned by `conformance/probes/variadic_pack_owned_iteration.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R34 `@explicit_destroy` is accepted on a type that is `Deinitable`
  unconditionally**

  Problem: Mojito accepts `@explicit_destroy` on `struct Ticket(Movable)`,
  while the pin rejects it: "@explicit_destroy is not valid on `struct` with
  unconditional conformance to `Deinitable`".
  - Mojito accepts a program the pin rejects, so this is a divergence.
  - The pin wants a `Deinitable where False` conformance beside the
    decorator, which every bundled explicit-destroy struct already spells.
  - Probe: `conformance/probes/explicit_destroy_without_deinitable_opt_out.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R48 A `String` element copied out of a `Span` fails**

  Problem: `String(span[0])` or `span[0].copy()` over a `Span[String, _]`
  traps in the VM with "invalid reference projection Field("_data") on
  None", and a later use of the span is refused as "access to 'items'
  conflicts with live reference 'span'"; the pin runs both.
  - An `Int` span, `String(items[0])` on the list, and `span[0] + "y"` all
    run, so the gap is a whole-value read of a nominal element through the
    view.
  - Pinned by `conformance/probes/span_string_element_copy.mojo`.
  - Found while deriving an annotated view binding; not root-caused.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R49 A generic method moving its `var` parameter into a sibling call's
  `mut` local argument is rejected**

  Problem: `self.fill(extra, value^)` in a method of `Bag[T]`, where `fill`
  appends `value` into its `mut target: List[Self.T]`, reports "use of
  uninitialized value 'value'", where the pin runs it and prints `1`.
  - The non-generic struct, and a module-level generic `def` called from
    `main`, both run.
  - The template's replay of `fill`'s summary roots the loan at `value`'s own
    place (`replay_transfer_effects`, the registration in
    `declarations.rs`), and the erased body's check then reads the moved
    binding through that loan.
  - Pinned by
    `conformance/probes/generic_method_transfer_into_mut_local.mojo`.
  - The same report comes from `self.items.append(value^)` inside
    `try`/`finally`, and so under any `with` statement, from the template's
    own check even when the method is never called; without the `try` it
    runs. Pinned by `conformance/probes/generic_method_transfer_in_try.mojo`.
  - Divergence R167 (`moved-parameter-into-local-collection`) is the same
    stand-in place and closes with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R54 A literal passed to a `ref` parameter stops at run time**

  Problem: `look(3)` against `def look(ref other: Int)` is accepted and then
  fails with "reference binding to a non-place expression".
  - The pin materializes the literal and binds the parameter to the temporary.
  - Probe: `conformance/probes/literal_to_ref_parameter.mojo`.
  - It materializes through the same fallback divergence R169
    (`ref-binding-register-value`) wants narrowed, so the two answers must
    agree.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R274 A subscript of a list literal stops at run time**

  Problem: `print([1, 2][0])` prints `1` at the pin and stops in Mojito with
  "reference binding to a non-place expression".
  - `List.__getitem__` borrows its receiver, and a list literal is no place;
    a call result (`make()[1]`) is materialized as a temporary and works.
  - The reflection crossing folds `r.field_names()[i]` to its element, so
    only a written literal reaches this.
  - Probe: `conformance/probes/list_literal_subscript.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R87 A loan carried only by a container's element type does not keep
  its source alive**

  Problem: `var s = l[0].copy()` over `l: List[Span[Int, origin_of(xs)]]`,
  then `print(s[1])` after the last direct use of `xs`, stops with "use after
  Pointer deallocation" on the VM.
  - `s`'s own type names `origin_of(xs)`, but nothing extends `xs` past its
    last direct use for it.
  - The same happens when such elements are moved between two lists whose
    element origin is a union (`origin_of(xs, ys)`), and when a
    `Dict[Int, Span[…]]` is read through `items()`.
  - A later direct use of `xs` hides it, which is how
    `assets/ok/bundled_instance_loan_carrying_argument.mojo` stays clear of it.
  - An untracked pointer's element type carries it the same way: a user
    `mk[Span[Int, origin_of(xs)]](1)` returning `Pointer[T,
    MutUntrackedOrigin]` reads freed memory after `xs`'s last use.
  - Both the erased body and the derived clones behave the same, so the
    lever is the caller-side loan a type-carried origin installs, not the
    callee.
  - Not yet checked against the pin, which is expected to print `2`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R88 A copy of a loan-carrying value is taken as a borrow of its
  receiver**

  Problem: a method returning a struct that carries a loan through a type or
  origin argument (`List[T].copy()` at `T = Span[Int, origin_of(xs)]`,
  `Span.copy()`) records a view-result borrow of its receiver, so the copy
  looks like it aliases what it was copied from.
  - `l.extend(l.copy())` over `List[Span[Int, origin_of(xs)]]` is rejected
    as aliasing `self` and `other`, and `v[0].copy()` with `v = Span(l)`
    conflicts with the live `v`.
  - A concrete clone check at such an argument rejects `List.__imul__`'s
    `self.extend(orig.copy())` the same way. The clone derives in a normal
    compilation, but `MOJITO_VERIFY_TEMPLATE_FACTS=1` checks it again and
    fails there.
  - The same borrow is why verification disagrees with the derived bodies
    of `Bag.get` in `assets/ok/loan_carrying_instance_clone.mojo` and of
    `List._get_copy`: the template records no borrow.
  - The rule is the `BorrowViewResult` condition in `method_calls/call_contract.rs`: any
    loan-carrying struct result of a non-consuming method.
  - Dropping the borrow when the declared return names only the struct's own
    origin binders was tried. It lost the only link from `c.get()`'s result
    to the loans `c` carries, which R87 has to supply first.
  - Not yet checked against the pin, which is expected to accept both
    spellings.
  - Depends on R87.
  - Model: Fable, Planned.

- [ ] **R89 A subscript of an element read through a `Span` of `Span`s fails
  on the VM**

  Problem: `var v = Span(l)` over `l: List[Span[Int, origin_of(xs)]]`, then
  `print(v[1][2])`, stops with "invalid reference projection Field("_data")
  on None".
  - Binding the element first (`var second = v[1]`, then `second[2]`) runs.
  - The same failure shows through a method returning such a view
    (`Bag.view()` in
    `assets/ok/template_view_over_loan_carrying_argument.mojo`, which binds
    the element first).
  - Not yet checked against the pin, which is expected to print `3`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R93 A loan-carrying element read out of a list does not keep its
  loans**

  Problem: `var x = spans[0]` or `var x = spans.pop()` over `spans:
  List[Span[Int, origin_of(local)]]`, then `local.append(1)` and `x[0]`,
  checks, and the VM stops with "use after Pointer deallocation".
  - The ownership check should reject the mutation of `local` while `x`
    lives, as it does for a view that borrows `local` directly.
  - The element's type names `local`'s origin, but the read records no loan
    for it: a result's loans come from a view-result adjustment, and an
    element is not a view of its container.
  - Before `List.pop` stopped lending its receiver (2026-09-27), the `pop`
    form was accepted and happened to print the stale element.
  - Not yet checked against the pin, which is expected to compile both.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R94 A pointer field of a struct read out of a list cannot be
  subscripted through**

  Problem: `var x = boxes[0].copy()` (or `boxes.pop()`) over `boxes:
  List[RefBox[origin_of(local)]]`, whose `RefBox` holds `var value:
  Pointer[List[Int], Self.origin]`, then `print(x.value[][0])`, stops with
  "vm: checked nominal subscript receiver is None".
  - The same read on a `RefBox` built directly runs.
  - Not yet checked against the pin, which is expected to print the element.
  - Found while closing `List.pop`'s view-result disagreement (2026-09-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R101 An initializer call is not judged by argument aliasing**

  Problem: `Tuple(Span(xs), Span(xs))` over a `var xs` runs and prints
  `3`; the pin rejects it with "aliasing values passed mutably to 'args'
  argument and passed mutably to 'args' argument in 'Tuple[...]'
  initializer call".
  - Neither the place rule (`check_call_aliasing`) nor the carried-origin
    rule (`check_argument_origin_exclusivity`) runs on a constructor call;
    both run on free calls, methods, and static methods.
  - A tuple literal is judged as its initializer's call already
    (`check_tuple_literal_exclusivity`).
  - `conformance/probes/initializer_arguments_share_mutable_origin.mojo`
    pins it.
  - Found while judging variadic elements in argument exclusivity
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R103 A tuple literal holding a pointer to a local is rejected**

  Problem: `var p = (Pointer(to=x), 2)` is rejected with "not a
  compile-time value: type pack contains a type which cannot be
  materialized in source"; the pin prints `p[1]`.
  - `(Span(xs), Span(xs))` over a read parameter materializes, so the
    pointer's origin spelling is the likely difference.
  - `conformance/probes/tuple_literal_pointer_to_local.mojo` pins it.
  - Found while judging variadic elements in argument exclusivity
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R104 A type mismatch between two loan-carrying types prints them
  alike**

  Problem: `two(Span(xs), Span(ys))` over `def two[T: Copyable](a: T, b:
  T)` or `(*args: T)` is rejected, as the pin rejects it, but with "type
  mismatch for type parameter 'T': expected Span[Int], found Span[Int]".
  - The pin names the origins: "cannot be converted from 'Span[Int,
    origin_of(ys)]' to 'Span[Int, origin_of(xs)]'".
  - `Ty`'s display drops a struct's origin arguments, so every mismatch
    that differs only in an origin reads as no mismatch at all.
  - Found while binding a homogeneous `*args` element's origins
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R111 An evaluated default handed to a borrowing parameter is
  never destroyed**

  Problem: `use()`, beside `def use(r: R = R(String("dflt")))` over a
  struct with a `__deinit__`, never runs that destructor on the VM or
  natively, where the pin prints `drop dflt` when the call returns.
  - An explicitly passed temporary (`use(R(String("x")))`) is destroyed
    on both sides, and so is the default of a `var` parameter, which the
    callee owns.
  - The VM's `bind_for_call` runs the default function into a register
    no drop elaboration sees. Native lowering matches it, and frees the
    value's buffers without running the destructor.
  - Probe: `conformance/probes/borrowed_default_argument_destructor.mojo`.
  - Found while evaluated defaults were lowered natively (2026-09-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R114 An implicit conversion through a consuming constructor copies a
  parameter-typed place**

  Problem: `self.keep(self.item)` with `keep(self, box: Wrapper[Self.T])` and
  `@implicit def __init__(out self, var value: Self.T)` runs for
  `T: Copyable`, copying a `List[Int]` implicitly, where the pin rejects the
  declaration.
  - The pin reports "value of type 'T' cannot be implicitly copied, it does
    not conform to 'ImplicitlyCopyable'".
  - Mojito's check of the method records no copy at the conversion's
    consuming parameter.
  - Probe: `conformance/probes/consuming_conversion_copies_parameter.mojo`.
  - Found while the template's MIR replaced per-instance method clones
    (2026-10-01).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R115 A second `Dict` lookup stales a view of an earlier value**

  Problem: `var v = d["a"].rstrip()` then `var u = d["b"].rstrip()` then a read
  of `v` is rejected with "use of invalidated interior reference 'v' to
  'd["value"]["bytes"]'", where the pin prints both.
  - A `Dict` lookup defines a fresh `value` generation, which replaces the
    earlier one (`record_replacing_interior_reference`).
  - The view lends `d["value"]["bytes"]` below the earlier generation, so the
    second lookup stales it.
  - The pin names the same interior, and a lookup does not replace it there.
  - A single view of a `Dict` value runs
    (`assets/ok/list_element_view_method_result.mojo`).
  - Probe: `conformance/probes/dict_value_view_second_lookup.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R116 A `^` transfer of a `ref` binding fails in MIR verification, not
  in the checker**

  Problem: `ref r = s` then `var t = r^` is rejected with "unsupported
  specialized MIR that does not verify: ... binding of ref String to a slot
  of type String", where the pin reports "expression does not designate a
  value with an origin".
  - A borrowed loop binder (`for x in xs: var y = x^`) fails the same way.
  - The program is rejected on both sides, so only the diagnostic and the
    phase that owns it are wrong.
  - `check_transfer_source` (`checker/places.rs`) skips a binding of
    reference type, and is where the rejection belongs.
  - Whether a `ref` parameter over a mutable origin may be transferred is
    still to be observed on the pin.
  - Probe: `conformance/probes/reference_binding_transfer.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R117 Converting a `List` to a `Span` copies its elements**

  Problem: `var s: Span[Dup, origin_of(xs)] = xs` runs each element's copy
  constructor twice, where the pin builds the view and copies nothing. The
  copies are never destroyed.
  - An element read through the view (`print(s[0])`, `g(s[0])`) copies once
    more, where a `List`, `Tuple` or pack element is read in place.
  - The view read keeps the copy because lending it takes the path R48
    reports as failing.
  - The conversion's copies are not root-caused.
  - Probe: `conformance/probes/span_conversion_copies_elements.mojo`.
  - Depends on R48.
  - Model: Opus, Not Planned.

- [ ] **R118 An element of a temporary container cannot be passed to a read
  parameter**

  Problem: `g(make()[0])` over `def make() -> List[Dup]` is rejected with
  "value of type 'Dup' cannot be implicitly copied", where the pin lends the
  element and destroys the temporary after the call.
  - The element of a named container is lent (`g(xs[0])`).
  - The checker treats the reference result of a temporary receiver as an
    ordinary value read.
  - Workaround: bind the container to a local first.
  - Probe: `conformance/probes/temporary_element_read_argument.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R120 A `mut` capture may be transferred away and written back**

  Problem: `def inner() {mut s} -> String:` whose body runs `var r = s^`,
  `s = String("b")`, `return r^` prints `ab` in Mojito, where the pin reports
  "cannot consume indirect references to values".
  - The pin refuses any `^` out of a capture, written back or not.
  - Mojito treats a capture like a `mut` parameter: left empty it reports
    "'s' is uninitialized at return from this function", written back it runs.
  - The rule belongs in the checker beside `check_transfer_source`.
  - Probe: `conformance/probes/mut_capture_transfer_refilled.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R121 A `TrivialRegisterPassable` struct cannot be transferred out of
  a `mut` parameter**

  Problem: `def take(mut v: V) -> V: return v^` for a
  `struct V(TrivialRegisterPassable)` prints `3 3` at the pin and is rejected
  in Mojito with "'v' is uninitialized at return from this function".
  - The pin transfers a trivial register value as a copy and only warns that
    the `^` has no effect.
  - MIR lowering copies only values whose type owns no storage
    (`owns_droppable_storage`), and every struct counts as owning.
  - Lowering has no conformance facts. The checker knows
    (`is_trivial_register_passable`) and would have to record it on the
    transfer.
  - Probe: `conformance/probes/trivial_struct_mut_parameter_transfer.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R122 A pointer handed to a struct's constructor does not keep its
  pointee alive**

  Problem: `var c = Cell[Pointer[List[Int], ImmOrigin(origin_of(xs))]](p)`
  followed by `print(c.item[][1])` prints 2 at the pin and stops in Mojito
  with "use after Pointer deallocation".
  - The cell takes no loan on `xs`, so `xs` is destroyed after the pointer's
    own last use.
  - A copy read back out (`var q = c.get()`) loses it the same way.
  - A method that stores the pointer after construction does keep it
    (`assets/ok/template_served_loan_carrying_instance.mojo`). A constructor
    body records no transfer, and the caller's aggregate path does not see a
    copied pointer.
  - Probe: `conformance/probes/pointer_copied_into_struct_keeps_loan.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R123 A heap-owning temporary that carries a loan is destroyed twice
  when an owning parameter takes it**

  Problem: `take(Maybe(Span(xs)))` with `def take[T](var m: Maybe[T])` prints
  1 at the pin and stops in Mojito with "use after Pointer deallocation".
  - MIR lowering anchors the temporary in a hidden `$arg_loan_r` slot to keep
    `xs` alive through the call (`anchor_borrowing_argument`,
    `mir/calls.rs`).
  - The slot and the callee's parameter both own the value, and both destroy
    the list inside it.
  - A temporary with no destructor (`Span(xs)`) is unaffected.
  - Probe: `conformance/probes/loan_carrying_temporary_owned_argument.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R124 A struct argument over an immutable origin counts as mutable
  in the exclusivity check**

  Problem: `b.push(s)` with `s: Span[Int, ImmOrigin(origin_of(xs))]` on a
  `Bag` over the same type prints `1 2` at the pin and is rejected in Mojito
  with "aliasing values passed mutably to 'self' argument and passed mutably
  to 'value' argument".
  - The same call over `Pointer[List[Int], ImmOrigin(origin_of(xs))]` is
    accepted.
  - The check reads a struct's origin argument without the `ImmOrigin` cast's
    mutability.
  - Probe:
    `conformance/probes/immutable_origin_struct_argument_exclusivity.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R125 `len` of a list reached through a pointer field does not run**

  Problem: `print(len(c.item[]))` with `c.item` a
  `Pointer[List[Int], ImmOrigin(origin_of(xs))]` prints 3 at the pin and
  stops in Mojito with "vm backend does not support methods on ref yet".
  - The field read yields a reference handle the builtin does not read
    through.
  - Probe: `conformance/probes/len_of_dereferenced_pointer_field.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R126 A clone over an origin binder loses the loan of a copy it
  stores**

  Problem: `s.put(p, 3)` with `def put[U](mut self, value: Self.T, tag: U)`
  storing `value.copy()` prints `3 1 8` at the pin and stops in Mojito with
  "checked nominal subscript receiver is None".
  - A method with a parameter of its own runs as a per-call clone, spelled
    over a clone origin binder where the instance's argument carries a loan.
  - The clone's stored type is closed, so its body publishes no carried
    source, and the copy names no argument expression. The receiver takes no
    loan on `xs`, which is destroyed after its last naming.
  - A generic `def` that keeps its clone at a loan-carrying argument loses
    the loan the same way.
  - A body its template serves keeps it
    (`assets/ok/template_served_def_loan_carrying_argument.mojo`).
  - Probe: `conformance/probes/per_call_clone_copied_loan.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R129 A conditional `__deinit__` is accepted where the struct's
  `Deinitable` conformance is unconditional**

  Problem: `def __deinit__(deinit self) where IsTriviallyDeinitable[Self.T]`
  in `struct Holder[T: Copyable & Deinitable](Copyable, Movable)` runs in
  Mojito, and the pin rejects the struct: "'Holder[T]' does not implement
  all requirements for 'Deinitable' ... lacking evidence to prove
  correctness".
  - The elaborator leaves the `__deinit__` out of `Holder[String]`, whose
    clause fails, so that instance is destroyed without it.
  - The checker should demand that a struct's `Deinitable` conformance be at
    least as conditional as its `__deinit__`, as the pin does.
  - Probe:
    `conformance/probes/conditional_deinit_without_conditional_conformance.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R133 A `Pointer[T, o]` parameter cannot infer both its element type
  and its origin**

  Problem: `def idp[T: AnyType, o: MutOrigin](v: Pointer[T, o])` called with
  `p = Pointer(to=x)` stops with "type mismatch for argument to 'idp':
  expected Pointer[Int, origin#1], found Pointer[Int, origin@…]"; the pin
  runs it.
  - The element binds, but the origin parameter stays an unsolved slot.
  - The same signature with a concrete element (`Pointer[Int, o]`) runs.
  - The parameter order does not matter.
  - Found while fixing a call-result pointer's dereference (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R136 An implicit copy at a binding's last use is not turned into
  a move**

  Problem: `var e = c` where `c` is never read again prints the
  `ImplicitlyCopyable` struct's `copy` in Mojito; the pin prints nothing.
  - The pin transfers a place at its last use instead of copying it; a
    chained comparison's middle operand (`b == d == c` as `c`'s last use)
    shows the same difference.
  - Mojito's ownership analysis knows each last use, but lowering has
    already chosen a copy, and a copy constructor with a side effect makes
    the choice observable.
  - Found while closing the chained-comparison copies (2026-10-03).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R266 A view of a whole local's owned interior is lent as the whole
  local**

  Problem: `var view = s.strip()` then `s = String("q")` then a use of
  `view` is rejected at the store as "access to 's' conflicts with live
  reference 'view'", where the pin rejects the use as "use of invalidated
  interior reference 's["bytes"]'".
  - The verdict agrees; the shape does not: the pin lends `s["bytes"]` and
    the store invalidates it, Mojito lends all of `s`.
  - A field receiver (`h.name.strip()`, `self.name.strip()`) already lends
    its named interior; a bare variable receiver still takes a whole-place
    loan in `aggregate_borrows_unmaterialized`'s view-result path.
  - Moving the owner (`var g = h^`) under such a view reports the stale view,
    where the pin reports "use of uninitialized value 'h'".
  - Found while closing the `self` field store under a live view (2026-10-04).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R302 A linear `Dict.deinit_with` leaves its emptied entries list to
  an implicit destruction its element type does not allow**

  Problem: `Dict[Int, Conn].deinit_with` over a linear `Conn` pops every
  entry and leaves `entries`, an empty `List[DictEntry[Int, Conn]]`, to the
  `deinit self` residual teardown.
  - That list's `__deinit__` is conditional on a `Deinitable` element, so the
    elaborator rightly mints none, and concrete MIR never frees the buffer.
  - The erased run calls the template's destructor and logs a `drop List`
    the concrete run lacks.
  - The checker accepts the implicit residual destruction of a field whose
    conditional `Deinitable` conformance the instance disproves.
  - Consuming `entries` through `List.deinit_with` is the stdlib lever, but
    the nested handler must forward the captured `elt_handler` and move both
    fields out of a `var` entry, which Mojito does not accept yet.
  - `assets/ok/dict_insert_linear_capable.mojo` is the
    `ERASED_VM_RESIDUE` row (`tests/corpus_test.rs`).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R304 A view copied out of a reference result carries no loan on the
  storage it views**

  Problem: `var v = first.value()` over `first = it.peek_next()` of a
  `String`'s codepoint iterator binds a `StringSpan` whose loan on the string
  is lost, so drop elaboration destroys the string first and `String(v)`
  stops with "use after Pointer deallocation"; the pin prints `h`.
  - The binding's loans come from `aggregate_borrows`, which sees no borrow
    for a value copied out of `Optional.value()`'s reference result.
  - Passing the same value straight to `print` works only because a view
    argument stays in its register (`bind_temporary_argument`).
  - Depends on nothing.
  - Model: Opus, Planned.

### Catch Up To Current Mojo: Calls, Overloads, Conversions, And Traits

Track: `calls`.

Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

- [ ] **R25 A user `Hasher` cannot spell `update` the way the pin requires**

  Problem: the `26cfe94f40` pin's `Hasher` requires
  `update(mut self, value: ImmSpan[Byte, _])`, and Mojito requires
  `_update_with_bytes(mut self, Span[Byte, _])` beside
  `update(mut self, Some[Hashable])`. A conformer cannot satisfy both.
  - The pin rejects `assets/ok/hasher_user_conformer.mojo` and
    `assets/ok/dict_hasher_forwarding.mojo` with "does not implement all
    requirements for 'Hasher'"
    (`conformance/assets-mojo-rejects.tsv`, family `hasher-protocol`).
  - Upstream's migration for a *call* is `value.__hash__(hasher)`, which both
    compilers already run; the three fixtures that only called `update` were
    respelled that way at the re-pin.
  - A sized scalar or vector now answers `v.__hash__(hasher)` through the
    checker's builtin hashable-leaf arm (`record_hash_leaf`), so the call
    side the rename needs exists; `Hasher.update` itself is still the
    checker's intrinsic, not a real `SIMD.__hash__` body.
  - Upstream also replaced the pointer-and-length `hash()` overload with
    `hash_bytes(ImmSpan[Byte])`, which lands in the same pass.
  - The levers are `checker/traits.rs` (the `Hasher` requirement set and its
    shape message), `checker/method_calls/intrinsic_receivers.rs` (the intrinsic arms),
    and `stdlib/std/hashlib/` (`hasher.mojo`, `_ahash.mojo`, `_fnv1a.mojo`
    plus every `hasher.update(...)` call site).
  - It changes a compiler-known trait's contract.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R33 A module-level `def` converts to a `def(...) capturing[_]` runtime
  parameter**

  Problem: `apply(handler: def(element: Int) capturing[_], /)` called as
  `apply(show)` with a module-level `def show(element: Int)` runs in Mojito;
  the pin rejects the call ("cannot be converted from `def show(element:
  Int) thin -> None` to `def(element: Int) capturing thin -> None`") and
  rejects a capturing closure there too ("capturing closures cannot be
  materialized as runtime values"), so such a parameter has no valid
  argument in the pin.
  - Probe: `conformance/probes/callable_parameter_thin_to_capturing.mojo`.
  - The pin does accept the same conversion into a generic struct's method
    for an `Int` instantiation (`Cell[Int].visit(show_int)`) and rejects it
    for `String`; the fixture that needed a callable parameter spells it
    `thin` (`assets/ok/template_method_callable_parameter.mojo`).
  - The bundled `List.deinit_with`, `Optional.deinit_with`, `Set.clear_with`,
    `Dict.clear_with`, and `DictEntry.reap_with` declare such parameters and
    are callable only through this conversion.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R35 A subscript store on a struct with a setter and no getter is
  accepted**

  Problem: `s[0] = 3` on a struct that declares `__setitem__` but no
  `__getitem__` runs in Mojito, while the pin refuses the store ("'Sink' has
  '__setitem__' but no '__getitem__' method").
  - The pin accepts the declaration itself while nothing subscripts it.
  - The lever is the setter selection in `check_nominal_subscript_assignment`
    (`checker/indexing.rs`), which never asks for a getter.
  - Fixtures that declare a setter alone need a getter first; the fallout is
    not enumerated.
  - Pinned by `conformance/probes/setter_without_getter.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R37 A trivial value handed to a `var` parameter beside a pack is always
  ambiguous**

  Problem: `g(x, x + 1, x)` against `g[*Ts](a: Int, *rest: *Ts)` beside
  `g[*Ts](a: Int, var b: Int, *rest: *Ts)` prints `3` at the pin, while Mojito
  reports an ambiguous call.
  - The pin selects the first overload for `Int(2)`, `v^`, and `True`, the
    second for `x + 1`, and calls a bare `2` ambiguous. No rule was recovered.
  - `VariadicBinding::bind` marks the case undecided, and an undecided tie is
    reported ambiguous, so Mojito never selects a different overload than the
    pin.
  - A place argument is settled: the implicit copy costs and both print `2`.
  - Pinned by `conformance/probes/pack_overload_var_trivial_undecided.mojo`.
  - The plan must find the rule with more probes, or move the entry to
    `docs/non-goals.md` as a kept over-rejection.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R38 A trivial rvalue handed to a `var` parameter beside a read overload is
  accepted**

  Problem: `q(x + 1)` against `q(var a: Int)` beside `q(a: Int)` is "ambiguous
  call to 'q'" at the pin, while Mojito selects the `var` overload.
  - The pin accepts the literal `q(7)`, the place `q(x)` and the transfer
    `q(x^)` in the same program, so only the non-literal trivial rvalue is
    ambiguous. No rule was recovered.
  - This is the non-variadic sibling of R37, and the two
    disagree: beside a pack the pin calls a bare literal ambiguous, here it
    accepts one.
  - `ArgumentBinding`'s `undecided` bit models the pack rule and is deliberately
    kept off non-variadic candidates, since applying it would reject `q(7)`.
  - Pinned by `conformance/probes/overload_var_trivial_rvalue.mojo`.
  - The plan must find the rule with more probes, or move the entry to
    `docs/non-goals.md` as a kept divergence.
  - Depends on R37, where the `undecided` rule is modelled: this one reuses
    the rule that entry recovers, or is withdrawn with it.
  - Model: Opus, Planned.

- [ ] **R39 A list literal beside a `List` parameter and a pack is not ambiguous**

  Problem: `g(x, [1, 2], x)` against `g[*Ts](a: Int, *rest: *Ts)` beside
  `g[*Ts](a: Int, b: List[Int], *rest: *Ts)` prints `2` in Mojito, while the
  pinned Mojo reports "ambiguous call to 'g'".
  - Mojito charges the literal's conversion to the second overload only. A
    string literal is already charged on the pack side too
    (`pack_element_conversion_count`).
  - Charging a list literal one conversion on the pack side does not tie the
    candidates, so the regular binding costs something else first.
  - Mojito accepts a program the pin rejects, so this is a divergence.
  - Pinned by `conformance/probes/pack_overload_list_literal_ambiguity.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R44 A bound call's result converted at an annotated binding fails
  MIR verification**

  Problem: `var x: Optional[T] = v.copy()` in a generic `def` reports
  "register r1 has no checked type" for the `copy` call, where the pin runs
  it.
  - The same binding at a concrete type runs, and so does the result bound to
    an unannotated local first and then converted.
  - Probe: `conformance/probes/bound_call_result_converted_at_binding.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R45 A matching constructor clone displaces every generic constructor,
  not only its own template**

  Problem: on a struct specialized whole (a variadic one, as `Variant`), a
  call that a per-call constructor clone matches never considers a second
  generic constructor, even one that would rank better.
  - Such a clone is declared `__init__` beside its template, and selection
    drops every generic candidate once a clone matches (`ranks_beside_clone`,
    `checker/declarations.rs`).
  - The clone carries a mark (`MethodProvenance::PerCallConstructor`), not the template
    it was minted from, so the filter cannot drop that template alone.
  - No program is known to hit it. `Variant`'s two generic constructors never
    match the same call, and no fixture pins it.
  - A non-generic struct's clone is minted under its mangled name and reached
    by retargeting, so its overload set holds no clone and needs no filter.
    Minting the specialized struct's clone the same way removes the filter.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R50 A value-returning body that ends in `abort(...)` is rejected**

  Problem: `def f(x: Int) -> Int` whose last statement is `abort("no")` runs
  at the pin, while Mojito reports "'f' does not return a value on every
  path".
  - `conformance/probes/abort_ends_a_returning_body.mojo` pins it.
  - The return analysis (`stmt_returns`, `checker/declarations.rs`) is
    syntactic. It knows `return`, `raise`, and the compiler crossing
    `_mojito_abort`, not a call to the bundled `std.os.abort` that wraps it.
  - Under source validation a `comptime for` that holds a `return` defers the
    verdict to the unrolled clone for the same reason: `Bag.get` in
    `assets/ok/pack_element_rebind.mojo` ends in `abort(...)`. That deferral
    can tighten once this is fixed.
  - The lever is a checked fact that a callee never returns, read where the
    call resolves, not a name test.
  - The plan must say where the fact lives and what MIR emits after such a call.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R52 A binder is not inferred through a converting argument**

  Problem: `boxed(value)` for `def boxed[U: Copyable & Deinitable](box:
  Wrapper[U])` and a `value: T` reports "cannot infer type parameter 'U' of
  'boxed' from the arguments", where the pin infers `U = T` through the
  `@implicit` constructor and runs it.
  - Inference matches an argument's own type against the parameter's before
    any conversion is considered, so a conversion can only reach a parameter
    whose binders are already fixed (a method's `Wrapper[Self.T]`, or a
    closed type).
  - Pinned by `conformance/probes/inferred_binder_through_conversion.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R309 A vector argument does not infer both a `DType` and a width
  binder**

  Problem: `total(SIMD[DType.int32, 4](1, 2, 3, 4))` for `def total[dt:
  DType, w: SIMDLength](v: SIMD[dt, w])` reports "cannot infer type
  parameter 'w' of 'total' from the arguments", and a method of that shape
  finds no matching overload; the pin infers both and runs it, `Float64` as
  a width-one vector included.
  - A width binder alone over a fixed dtype (`SIMD[DType.int32, w]`) is
    inferred, as is a `DType` binder alone over `Scalar[dt]`.
  - Upstream's `SIMD[_, _]` parameter is this shape with both binders
    implicit, so the hasher's leaf waits on it (R308).
  - Found while landing R5 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R53 A list literal does not reach a `List` built over a binder**

  Problem: `count([1, 2])` for `def count[T: …](extra: List[T])` reports
  "cannot infer type parameter 'T' of 'count' from the arguments", and
  `Bag[Int]([1, 2])` for a fieldwise `var extra: List[Self.T]` reports
  "type mismatch for field 1 of 'Bag': expected List[Int], found List[T]";
  the pin runs both.
  - The literal is typed at the declared `List[T]` without its binder solved
    or substituted, so the field case fails even with `T` given explicitly.
  - Pinned by `conformance/probes/list_literal_to_generic_list.mojo`.
  - Found while deriving another struct's overloaded method; not root-caused.
  - Depends on R52, which lets a binder be solved through a conversion; the
    literal's element type reaches `T` the same way.
  - Model: Opus, Not Planned.

- [ ] **R55 A call through a `ref` to a callable value is rejected**

  Problem: `for f in fns: print(f(5))` and `ref g = fns[0]; print(g(1))` run at
  the pin over a function display, while Mojito reports "'f' has type ref
  def(Int) thin -> Int and is not callable".
  - Both thin and capturing elements are affected.
  - The indexed call `fns[0](5)` already works through element-call dispatch.
    A `ref`-typed callee has no such path.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R56 A method called on a borrowed comprehension binder loses its
  receiver**

  Problem: `[item.get() for item in ps]` over a `List[P]` local passes the
  checker and then stops with "call passed 0 args to 1-parameter function
  'P.get'", where the pin prints the results.
  - The same call in a runtime `for` over the list runs.
  - `w.byte_length()` over a `List[String]` local runs, and over a
    `List[String]` parameter fails the same way.
  - The binder is a reference into the list, so the lever is how lowering
    passes a comprehension binder's handle as a method receiver.
  - Pinned by `conformance/probes/comprehension_binder_method_call.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R58 `Self(...)` does not construct inside a method**

  Problem: `return Self(self.a, not self.b)` in a method of an
  `@fieldwise_init` struct reports "Undefined variable 'Self'", and so does
  the keyword form; the pin constructs the enclosing struct.
  - Naming the struct (`P(...)`, `G(...)` in a generic struct) constructs,
    by position or keyword, so the gap is resolving `Self` as a callee.
  - A static call through `Self` (`Self.plus(a, 1)` in a `@staticmethod`)
    is rejected the same way, where the pin prints `101`; spelling the
    struct (`W[Self.k].plus(a, 1)`) works.
  - Pinned by `conformance/probes/self_call_construction.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R59 Explicit type arguments on a static method of a non-parametric
  struct are rejected**

  Problem: `P.plain[Int](4)` on `@staticmethod def plain[T: Writable](x: T)`
  prints `1` at the pin, while Mojito reports "Undefined variable 'P'".
  - Any generic static on a struct without parameters is affected, whether
    or not its body holds a `comptime if`.
  - The inferred spelling `P.plain(4)` works.
  - The explicit spelling parses as `Invoke` over `Member(P, plain)`. The
    error is raised before the non-parametric static path in
    `checker/method_calls/type_receivers.rs` sees the call.
  - The plan must first find which pass infers the bare type name as a value.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R60 An associated alias is not constructible through a parameterized
  base**

  Problem: `Holder[7].Same()` for `comptime Same = Sized[Self.n]` reports
  `Undefined variable 'Holder'`, where the pin runs it.
  - The annotation spelling works: `var made: Holder[7].Same = Sized[7]()`.
  - The alias now binds the instance's value parameters
    (`associated_type_from_base`), so only the call path is missing.
  - Depends on R59, which finds the pass that reads a bare type name as a
    value; the alias call needs the same answer.
  - Model: Opus, Planned.

- [ ] **R61 A leading-dot static call cannot take its struct's parameters
  from the expected type**

  Problem: `return .start(n)` against an expected `Counter[Self.T]` fails
  with "cannot infer type parameter 'T' of 'Counter' from the arguments",
  where the pin runs it.
  - The contextual root is rewritten to the expected type's head name alone
    (`with_contextual_root`), so the call can solve the parameters only from
    its arguments.
  - Spelled `Counter[Self.T].start(n)` it runs on both.
  - Pinned by `conformance/probes/contextual_static_expected_parameters.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R63 An exact constructor overload loses to its generic sibling as
  ambiguous**

  Problem: `Tag[Int](3)` on a struct declaring `__init__(out self, n: Int)`
  beside `__init__(out self, n: Self.T)` prints `int` at the pin, while
  Mojito reports "invalid call to 'Tag': ambiguous overloaded constructor
  call".
  - Upstream picks the more specific signature. Mojito's construction
    selection scores the substituted `Self.T` parameter as an equal match
    (`select_method_overload`, `checker/declarations.rs`).
  - Every constructor candidate is ranked now, generic or not, so specificity
    can be one more rank term.
  - Such a family is also the one shape that collapses on the instance, so
    its constructor clone family stays withdrawn.
  - Found while closing the overloaded-constructor-family item; no fixture
    pins it yet.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R64 Methods cannot overload on the parameter convention alone**

  Problem: `m(self, var a: String)` beside `m(self, a: String)` prints `2`
  then `1` at the pin, while Mojito reports "'m' is already declared in this
  scope". A free function accepts the same pair.
  - `same_method_shape` (`checker/traits.rs`) compares parameter types and
    ignores conventions, so the second declaration reads as a redeclaration.
  - The lowered method name has no owned-parameter qualifier either, so the
    two would collide even if the checker admitted them.
  - Ranking already decides such a pair: the place costs the `var` candidate
    a copy, and an owned argument selects it at the tie.
  - Pinned by `conformance/probes/overload_convention_only_method.mojo`.
  - The plan must say what the lowered name gains, and whether trait
    requirements compare conventions the same way.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R67 A `mut self` witness does not conform to a read-`self`
  requirement**

  Problem: a struct declaring `def __hash__(mut self, mut hasher:
  Some[Hasher])` is rejected as not conforming to `Hashable` ("missing
  required operation"), while the pin accepts the conformance and runs it.
  - Trait conformance compares the witness's receiver convention with the
    requirement's exactly.
  - A bound dispatch's derivation relies on that equality today: it admits
    a witness only with the requirement's own receiver convention
    (`template_facts/bound_dispatch.rs:witness_binders`), and must judge the
    receiver again once a `mut` witness conforms.
  - Pinned by `conformance/probes/mut_self_hash_witness.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R80 A clone that is still checked re-ranks an overloaded call**

  Problem: the pinned Mojo binds a call inside a generic body once, while it
  checks the body, and Mojito ranks the overload set again for every
  instantiation that takes the clone check.
  - An instance derived from its checked template inherits the template's
    choice (`assets/ok/template_overload_binding.mojo`,
    `overload-bound-in-generic-body`), with a scalar local or a branch
    (`assets/ok/template_overload_binding_local.mojo`), with a local of
    the parameter type handed to the call
    (`assets/ok/template_def_value_local.mojo`), from a struct method
    (`assets/ok/template_overload_binding_method.mojo`), and for a generic
    struct's static on a spelled receiver, even at an instance that
    collapses its family
    (`assets/ok/template_method_collapsed_static_overload.mojo`).
  - A call through a bound whose witness is overloaded selects the member
    witnessing the requirement, as the pin binds it, even where a rival
    would outrank it on the argument
    (`tests/compiler_test.rs:template_method_requirement_witness_outranks_a_better_rival`).
  - What remains is any body still outside the derivation classes for
    another reason: its instances are checked again, and rank the set
    again. It closes as the `pmir` stages widen the classes.
  - Depends on R10, which deletes the derivation classes once every stage
    before it has widened them.
  - Model: Fable, Not Planned.

- [ ] **R81 Ordering a tuple of mixed element types is rejected**

  Problem: `(1, String("b")) < (1, String("c"))` reports "operator '<' is
  not defined for Tuple[Int, String] and Tuple[Int, String]"; the pin prints
  `True`.
  - The checker decides tuple ordering structurally
    (`operators.rs`, `tuple_order_compatible`) instead of dispatching
    `Tuple.__lt__`, and that rule refuses most element mixes:
    `(Tag(2), 1)` over a user `Comparable` struct and `(2.0, String("x"))`
    fail too, while `(String("b"), 1)` and homogeneous tuples pass.
  - Calling `p.__lt__(q)` directly works, and so does the derived
    `Tuple.__lt__` behind it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R91 Same-arity overloads of a generic method on a generic struct stop
  at run time**

  Problem: `b.pick(1.5, 4)` on `Box[Int]`, whose `pick[U]` has a second
  overload of the same arity, stops with "vm: unknown method 'Box.pick'".
  - The pin prints both results.
  - The call resolves to the instance clone's overload symbol with `U`
    spelled with its bounds (`Box.pick$y3:Int$ov$U$Copyable$Deinitable$Int`),
    which no declaration carries (`Box.pick$y3:Int$ov$U$Int`).
  - It never reaches the per-call clone (`Box.pick$y3:Int$y7:Float64`), and
    two overloads called at one `U` would mint the same per-call name.
  - A non-generic struct, or overloads of different arities, run.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R92 A struct's callable-value parameter is undefined inside its
  methods**

  Problem: a method of `struct Apply[callback: def(Int) thin -> Int]` that
  calls `callback(self.seed)` checks, then stops at run time with "Undefined
  variable 'callback'".
  - The same parameter on a module `def` runs: the call frame reifies it as a
    hidden local.
  - A struct's deferred slot is never reified into its methods' frames.
  - Whether the pin accepts the program has not been probed. If it rejects
    it, the fix is a checker error.
  - Found while probing deferred slots (2026-09-26).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R97 `and`/`or` over a non-`Bool` operand is rejected**

  Problem: `n or 2` over `n: Int` prints `2` at the pin, and `if n and True:`
  runs; Mojito rejects both with "operator 'or' is not defined for Int and
  Int".
  - The checker types `and`/`or` only over two `Bool` operands; a numeric
    scalar or a `Boolable` struct never reaches the `Bool(x)` truthiness
    conversion a bare condition now takes.
  - Same-typed operands yield that type at the pin (Python's value-producing
    `or`), so the fix is more than a condition mark.
  - Found while accepting a numeric condition (2026-10-03).
  - A width-1 bool lane (`a == b or c < d` over `UInt64`s) rejects the same
    way, though `if`/`while` accept one through the `Bool(x)` truthiness
    conversion. Workaround: nest the tests.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R102 A method's `*Ts` pack does not take a `Span` argument**

  Problem: `S().show(Span(xs), 1)` over `def show[*Ts: Copyable](self,
  *args: *Ts)` is rejected with "'Span[_]' is not concrete; use '[]' to bind
  missing parameters"; the pin runs it.
  - The same pack on a module `def` takes the span.
  - Once it binds, argument exclusivity already judges the method's
    collected elements, and the pin rejects `S().show(Span(xs), Span(xs))`
    over a `var xs`.
  - `conformance/probes/method_pack_span_argument.mojo` pins it.
  - Found while judging variadic elements in argument exclusivity
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R105 A plain overload beside a variadic one over the same element
  type stops at run time**

  Problem: `def total(by: Int)` beside `def total(*rest: Int)` checks, and
  then `total(3)` fails with "operator Add is not defined for Int and
  Tuple[Int]", where the pin prints both results.
  - Both overloads lower to `total$ov$Int`: the overload key spells a
    collector by its element type, so the MIR holds one function under two
    declarations (`emit-mir` reports "duplicate MIR function name").
  - Struct methods collide the same way; overloads that differ in arity or
    element type, or in a keyword-only name after the collector, do not.
  - The lever is `SignatureKey` (`crates/mojito-symbol/src/symbol.rs`),
    which must mark the collector on the declaration side and at every call
    side that builds a key (`method_lowered_name`, `callable_lowered_name`,
    `callable_contract_target`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R108 The bundled `List` is not `Defaultable`**

  Problem: `Tuple[List[Int], Int]()` is rejected ("constraint declared here
  evaluated to False"), where the pin prints the empty list's length.
  - Upstream's `List` declares `Defaultable`. The bundled one declares an
    empty initializer and not the conformance.
  - `Dict` and `Set` are to be checked against the pin with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R109 A `Writable` conformance whose only `write_to` witness is
  conditional compiles**

  Problem: `struct Box[T](Writable)` whose `write_to(self, mut writer:
  Some[Writer])` carries `where conforms_to(Self.T, Writable)` is accepted,
  where the pin rejects it ("does not implement all requirements for
  'Writable'").
  - `verify_builtin_conformance` accepts any struct for `Writable`, because
    a struct without a `Writer`-fed `write_to` writes through the reflective
    default.
  - The pin also reports the conditional witness ambiguous beside its
    generated default.
  - This is the one case left where a call through a bound finds no member
    witnessing its requirement (`traits.rs:requirement_witnesses`), so the
    instance is checked again.
  - Found while the ranking fallback of bound-witness selection was removed
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R112 An implicit conversion in a cloned generic method is selected
  again per instance**

  Problem: `var w: Wrapper[Self.T] = self.item` in a method holding a
  `comptime if` is rejected for `Box[Int]` ("ambiguous implicit conversion
  from 'Int' to 'Wrapper[Int]'") when `Wrapper` has `@implicit` constructors
  over `Int` and over `Self.T`, where the pin prints 2 for every instance.
  - The pin selects the constructor while it checks the method, where the
    source is `Self.T`.
  - A method the template serves agrees with the pin
    (`assets/ok/implicit_conversion_bound_on_declaration.mojo`). A method
    that still clones repeats the selection at the instance's types, in its
    clone check and in its derivation.
  - Probe: `conformance/probes/implicit_conversion_bound_on_declaration.mojo`.
  - Found while classifying instance obligations
    (`docs/notes/generator-contract.md`, row 20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R113 `==` through a bound reaches an `__eq__` over another type**

  Problem: `self.first == self.second` over `T: Equatable` stops in
  elaboration at `Pair[Money]` ("argument 0 of 'Money.__eq__' has type Money,
  declared Cents") when `Money` declares only `__eq__(self, other: Cents)`,
  where the pin prints True.
  - The pin takes `Equatable`'s default, the fieldwise comparison, as the
    witness. The declared `__eq__` serves only a direct `Money == Money`,
    which converts the operand.
  - Mojito synthesizes no default `__eq__`, so the witness is the declared
    one, whose parameter is another type.
  - Until the template served the instance, the clone check converted the
    operand and called the declared `__eq__`, which printed False where the
    pin prints True.
  - Probe: `conformance/probes/equatable_witness_of_another_type.mojo`.
  - Found while the template's MIR replaced per-instance method clones
    (2026-10-01).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R119 A generic constructor beside a declared one cannot be
  constructed on a struct with a value parameter**

  Problem: `P[Int, 3](7)` for `struct P[U: AnyType, n: Int]` declaring
  `__init__(out self, var a: String)` beside
  `__init__[T: Writable](out self, a: T)` prints `5` at the pin and fails in
  Mojito with "vm backend does not support the built-in or callee
  'P.__init__$ov$T$Writable' yet".
  - The same pair runs on a struct whose parameters are all plain type
    parameters (`assets/ok/generic_struct_generic_constructor_overload.mojo`).
  - Such an instance mints the constructor's clone with its other per-call
    clones (`generate_instance_clones`). An instance with a value, origin or
    callable-bounded parameter mints no clones (`instance_arguments`), so the
    request has no owner to mint it.
  - The erased path cannot run it either: `--erased` reports "checked
    constructor 'P.__init__$ov$T$Writable' is missing from MIR".
  - Probe: `conformance/probes/value_struct_generic_constructor_overload.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R127 A method cannot call a generic `def` declared after its
  struct**

  Problem: `def size(self) -> Int: return tally(self.items)` with
  `def tally[T: Copyable & Deinitable](items: List[T]) -> Int` declared below
  the struct prints `1` at the pin and stops in Mojito with "Undefined
  variable 'tally'".
  - The struct may be generic or plain.
  - The same `def` declared above the struct runs.
  - Probe: `conformance/probes/method_calls_later_generic_def.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R128 A `Tuple` over a type parameter needs no evidence the
  parameter is `Movable`**

  Problem: `def total[T: Writable](pair: Tuple[Int, T])` runs in Mojito and
  the pin rejects the signature: "lacking evidence to prove correctness ...
  needs evidence for 'conforms_to(T, Movable)'".
  - `Tuple`'s element bound is `Movable`; a type parameter spelled as an
    element must prove it by its own bounds or a `where` clause.
  - Found while probing closed calls of a generic `def`.
  - Probe: `conformance/probes/tuple_over_unbounded_parameter.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R283 Constructing a type parameter through a user trait's
  `__init__` requirement is unsupported**

  Problem: `return T(v)` in `def build[T: Make & Movable & Deinitable]`,
  where `trait Make` requires `def __init__(out self, v: Int)`, reports
  "unsupported feature", while the pin runs it.
  - The checker constructs a type parameter only through `()` and
    `Copyable`'s `copy=` initializer
    (`Checker::infer_type_param_copy_construction`).
  - The construction should bind its arguments to the bound trait's
    `__init__` requirement, and the elaborator respell it as the bound
    struct's constructor call, as `T(copy=x)` already is.
  - Found while landing `T(copy=x)` (2026-10-04).
  - Probe: `conformance/probes/type_param_trait_init_requirement.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R284 A `@fieldwise_init` initializer does not satisfy a trait's
  `__init__` requirement**

  Problem: `@fieldwise_init struct A(Make, Movable)` with one `var v: Int`
  field reports "declares conformance to trait 'Make' but is missing method
  '__init__'" when `Make` requires `def __init__(out self, v: Int)`, while
  the pin accepts the conformance.
  - The conformance check does not see the synthesized initializer.
  - Found while landing `T(copy=x)` (2026-10-04).
  - Probe: `conformance/probes/fieldwise_init_satisfies_trait_init.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Catch Up To Current Mojo: Compile-Time Parameters, Packs, And Reflection

Track: `comptime`.

Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

- [ ] **R289 An explicit type argument before an inferred pack binds the
  pack empty**

  Problem: `has[Int](7, "x", False)` against `def has[T: AnyType, *Us:
  Writable](*extra: *Us)` fails with "'has$mono$TInt$T$pack$' expects 0
  argument(s), got 3", while the pin prints the body's result.
  - The instance key names `T` and an empty pack: the explicit argument
    fills `T`, and the pack the call's arguments imply is never solved.
  - With `T` inferred from an argument (`has(1, 7, "x")`) the call runs.
  - Found while landing R62 (2026-10-04).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R319 A local type alias applied as a constructor in an unrolled
  `comptime for` body is an undefined variable**

  Problem: `comptime T = Int` then `print(T(3))` inside `comptime for i in
  range(2)` in a plain `main` fails with "Undefined variable 'T'", where the
  pin prints `3` twice.
  - The same alias used as an annotation works, and the same call outside a
    loop works, so the AST unroller (`Elab::unroll_comptime_for`) loses the
    alias's binding for a call position.
  - Found while planning R246 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R320 A `comptime for` directly over `reflect[P].field_names()` is
  rejected**

  Problem: `comptime for name in reflect[P].field_names():` fails with
  "'reflect[P].field_names()' is a compile-time list; bind it with
  'comptime' and index it", even in a plain `main` over a concrete struct,
  where the pin prints each field name.
  - The iterable path does not accept the reflection list unbound, while
    `comptime names = reflect[P].field_names()` indexed by a `range` loop
    runs.
  - Found while planning R246 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R325 A value pack read as a runtime value is rejected**

  Problem: `for v in values:` and `values[i]` with a runtime `i`, over
  `def f[*values: Int]`, fail with "struct 'Tuple' was not registered" and
  "expected a compile-time Int index", where the pin runs both over the
  pack's runtime `VariadicList`.
  - Such a `def` keeps its clone (`value_packs_read_as_parameters`), and
    neither the clone nor MIR builds the pack as a runtime list.
  - A template-served body would need MIR to materialize the parameter
    list (a `Const::Param` of list meta) as runtime storage.
  - Found while landing R318 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R326 An empty explicit application `f[]()` is a pointer dereference**

  Problem: `none[]()` over `def none[*vals: Int]() -> Int` fails with "an
  empty subscript ('value[]') is the pointer dereference", where the pin
  binds the pack empty and prints `0`.
  - The parser reads `name[]` as `EmptySubscript` whatever `name` denotes;
    the checker would type it as an application when the callee is a
    generic `def`.
  - Found while landing R318 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R313 A generic struct's method that still clones per instance has
  no clone for a `StringLiteral` instance**

  Problem: `Box("a").local_ty()`, on a `Box[T]` whose `local_ty` binds
  `comptime U = Self.T`, stops with "abort: Box.local_ty: unspecialized
  type-keyed method", while the pin prints `1 0` beside `Box(1)`.
  - The receiver is typed `Box[StringLiteral]`, and MIR holds the `Int`
    instance's clone of `local_ty` but none for `StringLiteral`, so the call
    reaches the template's stub; `Box(String("a"))` runs.
  - Only a method R312 still clones can hit it.
  - Probe: `conformance/probes/stub_method_string_literal_instance.mojo`.
  - Found while landing R305 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R296 A method local annotated with a struct pack element's type is
  rejected**

  Problem: `var value: Self.Ts[i] = Self.Ts[i]()` in a method's `comptime
  for` fails with "dependent type indexing requires a type-valued associated
  member" (`checker/type_resolution.rs`), while the pin prints each
  element's default.
  - A `def`'s own `Ts[i]` annotation resolves to the dependent element
    (`assets/ok/pack_element_alias_served.mojo`); the `Self.Ts[i]` spelling
    has no such form.
  - Found while landing R252 (2026-10-04).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R65 Upstream `DType` names with no Mojito dtype are rejected**

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
  - The bundled `struct DType` port, R160, rewrites the same table, so this
    lands first or folds into it.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R66 Small SIMD surface gaps the symbolic-lane probes found on concrete
  types**

  Problem: each of these runs at the pin on a concrete scalar or vector and
  is rejected by Mojito, so a symbolic template cannot license it either.
  - `len(v)` on a vector (`len_result_for_type` has no `Ty::Simd` arm),
    `abs`/`max`/`min` over a scalar alias (`is_numeric` excludes it),
    `Scalar[dt].MAX`/`.MIN`, and `**` on a lane.
  - `Float64.cast[...]()`: the canonical width-one `float64` is `Ty::Float64`
    and has no SIMD methods.
  - Natively only, `String(v)` of a multi-lane vector is an unsupported
    type where `print(v)` lowers (`pliron backend: unsupported type`).
  - On a symbolic lane only, `range(Scalar[dt](0), n)`, `to_bits()` without
    an explicit target, and a `DType.bool` mask's `fill=` at a symbolic
    width are reported at the template rather than deferred to the
    instantiation.
  - One site each.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R68 An imported alias in a cloned generic method's signature is an
  unknown type**

  Problem: `def feed(self, mut hasher: default_hasher)` on a generic struct
  reports "unknown type '__module$hasher$default_hasher'" for an instance
  that clones the method, while the pin runs it.
  - A method clones where its body holds a compile-time construct, or its
    instance's argument carries a loan. A method the template serves
    resolves the alias.
  - The same annotation resolves on a module-level `def`, on a plain
    struct's method, and through a local `comptime` alias of the import.
  - Workaround: spell the application (`AHasher[SIMD[DType.uint64, 4](0)]`),
    as `assets/ok/template_method_bound_witness_shapes.mojo` does.
  - Pinned by `conformance/probes/imported_alias_in_generic_method.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R69 An uncalled `rebind` method is checked for every instance**

  Problem: a method whose `rebind` does not hold for one instance of its
  struct (`rebind[Int](self.value)` on `Box[String]`) is refused with "type
  mismatch for rebind" even when no call reaches it; the pin judges only the
  instances a call reaches and runs the program.
  - The elaborator clones every method of an instantiated struct, and each
    clone discharges its `rebind` equality.
  - Under `comptime if Self.T == Int` the arm is dropped for `String`, so a
    keyed `rebind` is unaffected.
  - Pinned by `conformance/probes/rebind_method_uncalled_instance.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R70 A value-keyed `def` cannot forward its value to a keyed `def`**

  Problem: `def forward[n: Int](x: Int) -> Int: return keyed[n]() + x`, over
  a `keyed[n: Int]` holding a `comptime if`, fails elaboration with
  "compile-time call arity: generic 'keyed' requires compile-time parameter
  'n'"; the pin prints the sum.
  - A value-keyed `def` with no compile-time control flow runs erased, its
    value passed at run time, so no compile-time `n` reaches the call.
  - A type parameter forwarded the same way (`show[T](x)`) works, because
    the abstract body's call reaches each instance's clone.
  - Pinned by `conformance/probes/value_param_forwarded_to_keyed_def.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R71 A compile-time-keyed `def` cannot be passed as a function value**

  Problem: `apply(as_int, 3)`, where `as_int[T]` holds a `comptime if` or a
  `rebind` and `apply` declares a callable bound, runs at the pin and reports
  "Undefined variable 'as_int'" in Mojito.
  - The template is dropped or stubbed, and only its `$`-mangled clones carry
    a name; a bare reference resolves to neither.
  - The rejection is safe (no wrong answer), but the message names nothing
    the source wrote.
  - The plan must say which clone a bare reference names, and what the rejection
    says when none can be chosen.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R72 A member-led arithmetic type argument does not parse in an alias
  body or a call's brackets**

  Problem: `comptime Next = Sized[Self.n + 1]` is a parse error (`Expected ']'
  after a subscript`), where the pin accepts it.
  - A call's brackets stop the same way: `size[Self.n * 2]()` and
    `Counter[Self.length * 2 - 1](i)` in a method; `2 * Self.length - 1`
    parses.
  - The bracket is parsed as a runtime subscript, whose index grammar stops at
    the member access. `Sized[(Self.n + 1)]` and `Sized[0 + Self.n]` parse.
  - The same expression in annotation position (`var x: Sized[Self.n + 1]`)
    parses, and the checker already types it symbolically.
  - It is the standing Index-versus-TypeApply split; the plan decides whether
    the alias body re-parses as a type or the subscript grammar widens.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R73 An arithmetic `where` operand compiles for `def`s and struct methods
  only**

  Problem: `where n + 1 == m` needs its declaration's value parameters in
  scope when the clause compiles, and only a free `def` and a struct method
  open that scope first.
  - A trait method, a comptime alias, and a Bool-bodied predicate alias
    report `unsupported generic constraint operand`, as every declaration did
    before.
  - The lever is `push_param_scope` around each remaining
    `compile_where_clause` site (`checker/traits.rs`, `statements.rs`,
    `conformance.rs`).
  - The sites are known; the predicate alias also needs its substitution
    (`substitute_predicate`) to carry an expression.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R74 `comptime assert` is not parsed**

  Problem: upstream's `comptime assert conforms_to(FT, Hashable)` is a parse
  error at Mojito.
  - The pin treats the assertion as a proof: a preceding, same-block
    `comptime assert conforms_to(X, T)` licenses `T` on `X` exactly as a
    `comptime if conforms_to(X, T):` arm does (`docs/notes/param-expr-attributes.md`
    §Reflection queries), and a failing one reports "constraint failed" at
    the instance.
  - The checker's arm licensing (`conformance_arm_assumptions`) is the lever;
    the statement form would push the same atoms for the rest of its block.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R75 `reflect[T]` over a function's type parameter is rejected**

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
  - The plan names every elaborator path that evaluates a retained template
    body.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R76 `reflect[T]` of a non-struct type is rejected**

  Problem: the pinned Mojo answers `reflect[Int].field_count()` with 0, and
  Mojito rejects it with "requires a struct type".
  - `comptime/eval.rs` raises it for every reflection method over a type that
    is not a struct.
  - `conformance/probes/template_fallback_reflection.mojo` records the
    observation, made 2026-09-20.
  - Which handle methods answer for a scalar, and with what, needs a probe per
    method before the lever is chosen.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R269 `reflect[Self]` in a non-generic struct's method is rejected**

  Problem: `comptime r = reflect[Self]` then `r.field_count()` in a method of
  a plain struct prints the field count at the pin, while Mojito reports
  "not a compile-time value: unsupported compile-time type argument".
  - The elaborator walks a plain struct's method bodies with no binding for
    `Self`, so `reflect[Self]` has no subject.
  - Both the bound form and `comptime n = reflect[Self].field_count()` fail.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R278 `reflect[Self]` in a generic struct's method is rejected**

  Problem: `reflect[Self].field_count()` in a method of `struct Box[T]`,
  written there or inherited from a trait default under `comptime if
  reflect[Self].is_struct():`, prints the field count at the pin, while
  Mojito reports "not a compile-time value: unsupported compile-time type
  argument".
  - The elaborator's `type_from_anno` has no binding for `Type::SelfType`,
    so an instance clone cannot name its own type as a reflection subject.
  - The upstream reflective defaults (`Hashable.__hash__`,
    `Equatable.__eq__`) are written over `reflect[Self]`; inherited
    defaults are now struct methods, so this and R269 are what they wait on.
  - Probe: `conformance/probes/reflect_self_generic_struct_method.mojo`.
  - Depends on R269.
  - Model: Opus, Planned.

- [ ] **R279 A struct's associated value read as `Self.N` is rejected**

  Problem: `return Self.N` over `comptime N: Int = 3` in a struct's own
  method, or in a trait default it inherits, prints the value at the pin,
  while Mojito reports "'Self.N' is not a type parameter of the enclosing
  struct".
  - A bound parameter reads the member (`T.size` over `T: Fixed`), so only
    the struct's own spelling is missing.
  - Probe: `conformance/probes/associated_value_self_read.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R280 `comptime if conforms_to(T, X):` over a `def`'s parameter fails**

  Problem: `def tag[T: Movable](x: T)` holding `comptime if conforms_to(T,
  Copyable):` prints `copyable` at the pin, while Mojito stops with
  "invalid checked program: fn '$comptime$tag$0': register r0 has no
  checked type".
  - The template-served `comptime if` outlines its condition into a thunk,
    and the thunk's MIR carries `conforms_to` as an untyped call over type
    operands.
  - The same condition over `Self.T` in a struct method runs.
  - Probe: `conformance/probes/conforms_to_condition_generic_def.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R270 A module-scope reflection handle is rejected**

  Problem: `comptime R = reflect[Point]` at module scope, read in `main` as
  `R.field_count()` or through `comptime n = R.field_count()`, prints 2 at
  the pin, while Mojito reports "unknown type 'reflect'".
  - The same binding inside a function works.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R271 A `field_types()` result in a runtime position is rejected**

  Problem: `var t = reflect[Point].field_types()` compiles at the pin, while
  Mojito reports "type-valued or symbolic comptime values cannot materialize
  at runtime".
  - The crossing pass folds a runtime reflection query to its value's
    literal form, and a list of types has none.
  - Upstream's result is a `TypeList` value, which Mojito's checker types
    only in compile-time positions.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R77 A field's value cannot be read by reflection**

  Problem: `reflect[T].field_ref[i](x)` — the value of field `i` of `x` — is
  unsupported; the upstream hashing, equality, and writing defaults are
  written with it.
  - Mojito's defaults are Rust AST synthesis (`comptime/synth.rs`) and a
    lowering (`mojito-pliron/src/lower/print.rs`), so no bundled body needs
    it yet.
  - Under validation the result's type is the opaque field type
    (`types[i]`), which the arm-licensing rule already covers.
  - Depends on R74 for the upstream spelling of the defaults, and on R51,
    the body shape they are written in.
  - Model: Fable, Planned.

- [ ] **R78 A local compile-time type list is unknown in an instance's annotation**

  Problem: `var v: types[i] = ...` over `comptime types =
  reflect[T].field_types()` validates, but the instance reports "unknown
  type 'types'".
  - The elaborator resolves `f.T` over a bound handle (`resolve_reflected_type`,
    `comptime/eval.rs`) and nothing else in an annotation; `comptime FT =
    types[i]` then `var v: FT` is the working spelling.
  - Depends on R40 when the annotation sits in a `comptime for` body.
  - Model: Opus, Not Planned.

- [ ] **R79 A handle chain in a call's type argument is read as a value**

  Problem: `_unqualified_type_name[reflect[T].field_at[i].T]()` reports
  "expected a type, found a value" at the instance.
  - The elaborator's `resolve_reflected_param_arg` rewrites a `ParamArg::Type`
    only; the chain arrives as a value argument.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R82 A struct keyed on a `DType` named like a bundled binder is
  rejected**

  Problem: `struct T[dt: DType]` constructed as `T[DType.int8]()` reports
  "variadic struct 'T' requires explicit compile-time type arguments"; the
  pin runs it.
  - `K` and `V` fail the same way, and `H` reports "generic 'H' requires
    compile-time parameter 'dt'".
  - The same struct named `Keyed`, or keyed on an `Int`, runs, as does a
    plain struct named `T`.
  - The message comes from the bare-identifier arm of `mono.rs`
    (`resolves_top_template` and `struct_template`), so a bundled body's
    binder spelled `T`, `K`, or `V` presumably resolves to the user's
    template, which only a struct specialized whole per value keeps in the
    specialization registry.
  - Found while writing `assets/ok/template_value_keyed_struct.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R83 A parametric nested `def` named as a value reports its marker**

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
  - Depends on R71, which decides which clone a bare reference to a
    parametric `def` names; the message follows from that.
  - Model: Opus, Not Planned.

- [ ] **R84 A generic struct's `DType`-keyed method is never cloned
  without a discovery round**

  Problem: `holder.double[DType.int64](9)`, over `def double[dt:
  DType](self, a: Scalar[dt]) -> Scalar[dt]` on `struct Holder[T: Copyable
  & Movable & Deinitable]`, reports "invalid checked program: ... argument 0
  of 'Holder.double$y3:Int' has type Int64, declared Scalar[dt]"; the pin
  prints `18`.
  - The call keeps the per-instance method, whose `dt` lowers untyped,
    instead of retargeting to a per-call clone.
  - It runs once another inferred keyed call in the program (a free
    `twice(Int16(5))`) forces a discovery round, which is why
    `assets/ok/dtype_keyed_method.mojo`'s `Holder` case passes.
  - The same method on a plain struct clones without discovery.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R85 A method forwarding its own `DType` parameter to a sibling
  keyed method is rejected**

  Problem: `def via[dt: DType](self, a: Scalar[dt]) -> Scalar[dt]: return
  self.twice[dt](a)`, over a sibling `twice[dt: DType]`, reports "type
  mismatch for return: expected Scalar[dt], found Scalar[dt]"; the pin prints
  `14`.
  - The checker binds the sibling's explicit `dt` to a binder other than the
    caller's, so two same-spelled lanes compare unequal.
  - The same forward to a free keyed `def` (`helper[dt](x)`) runs
    (`assets/ok/dtype_keyed_method_forward.mojo`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R90 A compile-time-keyed `def` applied to a loan-carrying type is
  rejected**

  Problem: `tag[Span[Int, origin_of(xs)]]()` on `def tag[T: AnyType]() ->
  Int` whose body branches on `comptime if T == Int` stops with "type 'Span'
  expects 1 compile-time argument(s), got 2".
  - The same `def` without the `comptime if` runs, as does the keyed one at
    `Int`.
  - The elaborator re-resolves the keyed argument with its origin tail
    spelled as a type argument.
  - Not yet checked against the pin, which is expected to print the tag.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R98 A `UInt`-annotated `comptime` literal reads as an `Int`**

  Problem: `comptime U: UInt = 3` then `var u = U; u -= 4` prints `-1`
  where the pin prints `18446744073709551615`.
  - The elaborator inlines a module constant's value at every use, and an
    integer value materializes as a bare literal, so each use types as
    `Int` whatever the annotation says.
  - Sized-scalar, vector, and `Float64` annotations already bind at their
    declared type: their value folds to a typed SIMD constant
    (`comptime/elab.rs`, `StmtKind::Comptime`).
  - The fix is a `UInt` constant that materializes as `UInt(3)` at a value
    use without changing how a `UInt` value parameter is spelled.
  - Found while closing the annotated-constant task (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R99 A method call whose compile-time argument names the caller's
  value parameter fails MIR verification**

  Problem: `s.scale[k](5)` inside `def run_at[k: Int](s: Doubler)`, beside
  `def scale[n: Int](self, value: Int)`, stops with "register r6 has no
  checked type", where the pin runs it.
  - A free function called the same way (`twice[k](5)`) runs, and so does
    the method call with a literal argument (`s.scale[2](5)`).
  - The same failure meets a call through a bound (`T: Scaler`), so a
    requirement default reading the method's parameter
    (`checker/bound_defaults.rs`) cannot yet be shown spelled from a
    caller's parameter.
  - Found while accepting requirement defaults over a method's parameters
    (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R100 A generic nested `def` with a by-value capture does not
  compile natively**

  Problem: `def inner[k: Int]() {var x} -> Int` runs on the VM and prints
  what the pin prints, but natively it is refused: "generic retained
  callable `outer$inner` captures by value".
  - Native monomorphization turns each call into a direct call to the
    instance, passing a by-reference environment's places as the lifted
    body's leading arguments (`capture_arguments` in
    `native/mono/specializer.rs`).
  - A `var` or moved capture is a snapshot held only by the closure value,
    which the direct call no longer reads; the snapshot needs a home the
    call can reach, such as a hidden local copied at the declaration.
  - `conformance/probes/generic_nested_def_var_capture.mojo` pins it.
  - Found while compiling capturing generic nested `def`s natively
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R106 A variadic struct's pack bound is not enforced at an explicit
  application**

  Problem: `Row[Plain, Int]()` over `struct Row[*Ts: Movable & Defaultable]`
  compiles and runs though `Plain` is not `Defaultable`.
  - The pin rejects it: "'Row' parameter 'Ts' has 'Defaultable & Movable'
    type, but value has type 'AnyStruct[Plain]'".
  - A body that relies on the bound then runs a member no element supports:
    `self.storage[i] = Self.Ts[i]()` constructs a `List[Int]` element, which
    the bundled `List` does not declare `Defaultable`.
  - A plain generic struct's bound is enforced (`Box[T: Defaultable]`).
  - Found while `Tuple`'s default initializer moved into source
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R107 User code may spell the compiler-private `__RuntimeTuple`**

  Problem: `var storage: __RuntimeTuple[*Self.Ts]` in a user struct, and
  `__RuntimeTuple(*args^)` in its initializer, compile and run.
  - The pin has no such name, so the acceptance is Mojito's own.
  - A subscript of `__UninitStorage` is already refused outside the bundled
    library ("is compiler-private storage; use MaybeUninit from
    std.memory").
  - The lever is the same test on the declaring module, at the annotation
    and at the construction.
  - No fixture pins the acceptance.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R110 A nested `def` cannot read an enclosing function's
  `comptime` binding**

  Problem: `comptime n = 3` in `main`, read by a nested `def inner() ->
  Int: return n + 1`, is rejected ("Could not infer capture convention of
  the captured value n"), where the pin prints `4`.
  - A `comptime` float binding is rejected the same way.
  - The binding is a compile-time value, as an enclosing function's value
    parameter is, and a nested def already reads one of those without
    naming it (`assets/ok/nested_def_reads_enclosing_parameter.mojo`).
  - Probe: `conformance/probes/nested_def_reads_local_comptime.mojo`.
  - Found while a local `comptime` float was made to read as `Float64`
    (2026-09-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R130 A struct member's `where` clause over `TypeList[Self.Ts.values]()`
  is rejected**

  Problem: `def has_int(self) -> Bool where
  TypeList[Self.Ts.values]().contains[Int]()` on `struct Row[*Ts: AnyType]`
  runs at the pin and stops in Mojito with "TypeList[...] takes a pack
  projection ('Ts.values')".
  - The parser reads the bracket argument in type position, and
    `typelist_receiver` (`checker/constraints.rs`) accepts only the value
    spelling `Ts.values`.
  - Accepting the type spelling there compiles the clause, but the call then
    judges `.all[IsTriviallyCopyable]()` false on `Row[Int, Bool]`, so the
    pack's binding at a method call needs a look too.
  - `conforms_to(Self.Ts.values, Copyable)` in the same position runs.
  - The elaborator decides `.all`, `.any`, `.contains`, and `.length` over a
    bound pack. An `.all` or `.any` through a predicate alias stays
    undecided there, because the alias's body does not reach MIR.
  - Probe: `conformance/probes/typelist_self_pack_member_clause.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R131 A compile-time evaluation cannot call a generic struct's method
  that holds a `comptime if`**

  Problem: `comptime CAP = np(5)`, where `np` calls `Cell[Int](n).m()` and
  `m` holds a `comptime if`, prints at the pin and stops in Mojito with
  "Cell.m: unspecialized type-keyed method".
  - The evaluation's subprogram carries `m` as its template stub and mints
    no per-instantiation clone for it, as the production elaboration does.
  - The same struct beside an evaluation that does not call `m` runs.
  - Entry R7 moves the evaluation onto the elaborator's worklist by the
    request path of `docs/notes/ctfe-request-path.md`, which serves the
    instance and closes this.
  - Probe: `conformance/probes/ctfe_calls_comptime_if_struct_method.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R132 A compile-time evaluation cannot call a value-keyed generic
  `def`**

  Problem: `comptime R = rep[3]()`, where `rep[n: Int]` recurses under a
  `comptime if n == 0`, prints `3` at the pin and stops in Mojito with
  "unknown compile-time function 'rep'".
  - The evaluation's subprogram excludes every compile-time-keyed `def`
    (`is_specializable_declaration_in`), so the call finds no body.
  - The same recursion under a runtime `if` runs in Mojito, on the erased
    body, where the pin expands without end; that is divergence R162
    (`ctfe-plain-keyed-recursion`).
  - Entry R7 closes this: a demand serves `rep[3]`, `rep[2]`, `rep[1]`,
    `rep[0]` from the worklist, each with its arm decided.
  - Probe: `conformance/probes/ctfe_keyed_recursion.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R134 `type_of(x)` is not accepted as a type**

  Problem: `var y: type_of(x) = 3` stops with a parse error at the `(`, and
  `unsafe_alloc[type_of(px)](1)` with "expected a type, found a value"; the
  pin runs both.
  - Spelling the type out (`unsafe_alloc[Pointer[Int, origin_of(x)]]`)
    runs.
  - Found while fixing a pointer dereference through a generic struct's
    field (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R135 A `comptime` constant built by a sized scalar constructor is
  rejected**

  Problem: `comptime K = Int32(4)`, at module scope or in a function, stops
  with "'Int32' is not a compile-time-callable function"; the pin binds it
  and prints `4`.
  - The elaborator folds a scalar construction only for `Int`, `UInt`,
    `Float64`, and `Bool` (`scalar_type_name` in `comptime/eval.rs`); any
    other call falls through to CTFE of a user function by that name.
  - `Int64`, `UInt8`, `Float32`, and the other `Scalar[dt]` aliases reject
    the same way.
  - Found while probing a scalar splat into a vector default (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R137 Checking a deeply nested generic struct type takes time
  exponential in its depth**

  Problem: `var a16 = W[W[…W[Int]…]](a15.copy())`, sixteen levels of
  `struct W[T: Copyable & Deinitable]` built one per line, takes minutes in
  `mojito check`, and every two more levels roughly triple the time.
  - Twelve levels take about 1.7 s beyond the check's fixed cost.
  - `wrap(wrap(…wrap(7)…))` over `def wrap[T](x: T) -> W[T]` grows the same
    way, about doubling per level from twelve on.
  - Declaring the same type without constructing it costs nothing, and
    nested calls of a generic `def` returning `T` stay flat.
  - A likely cause is a conformance of each level re-derived once per bound
    of the level below, with no memo.
  - Found while checking nested instance names (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R138 An applied module constant is not a struct's own type
  argument**

  Problem: `comptime M = f(7)` shapes a vector width as the application
  (`SIMD[DType.float32, M]` is `SIMD[DType.float32, f(7)]`), but a struct
  argument over it is not that type.
  - `Buf[Int, M]` is rejected ("cannot be a struct argument yet"): a struct
    instance below the waist is named by a constant argument, and a
    residual value argument names no instance.
  - `Buf[M]`, a single bracket argument, parses as a subscript, and the
    elaborator folds `M` to `8` in it, so Mojito accepts `Buf[8]` where the
    pin rejects it against `Buf[f(Int(7))]`.
  - The lever is the request path: an instance keyed by an evaluated
    application names it by the evaluation, as the pin's `(FuncOp,
    operands)` key does.
  - Found while landing the register types over parameter expressions
    (2026-10-03). The request path landed with the `comptime if` entry the
    same day (`Specializer::demand_application`).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R139 A module constant applying two functions, or a function with
  a non-`Int` result, keeps the folded value's identity**

  Problem: `comptime D = g(A) * h(A)` and `comptime E = pick()` returning a
  `DType` fold to their values before the check, so a type over them
  matches the folded value, which the pin rejects.
  - Source validation builds the application for a constant whose
    initializer applies one module `def` with an `Int` result, or arithmetic
    over such constants (`Checker::applied_constant_expr`); the executable
    check attaches the folded value to that one application
    (`Checker::evaluated_application`), and a node with two applications has
    no value for either.
  - The lever is the request path: each application is evaluated by the
    elaborator, and the fold goes with the AST route.
  - The elaborator evaluates an application that is a whole operand of a
    `comptime if` condition (`f(n) == True`, `size_of[T]()`); one nested in
    an arithmetic operand (`f(n) + 1 > 2`) is a `MonoError`
    (`Specializer::resolve_application`).
  - Found while landing the register types over parameter expressions
    (2026-10-03).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R272 A reflected field name materializes as `String`, not
  `StringSpan[ImmStaticOrigin]`**

  Problem: `materialize[names[i]]()` over `comptime names =
  reflect[T].field_names()` is a `String` in Mojito and a
  `StringSpan[ImmStaticOrigin]` at the pin, so the crossing diagnostic
  spells `Array[String, Int(2)]` where the pin spells
  `Array[StringSpan[ImmStaticOrigin], Int(2)]`.
  - The elaborator answers `field_names()` with a list of plain compile-time
    strings (`comptime/eval.rs::eval_reflection_method`), which materialize
    as `String` literals.
  - A program that annotates the element as a `StringSpan` is rejected where
    the pin runs it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R303 A capturing generic callable parameter cannot be applied at
  its own compile-time arguments**

  Problem: `callback[2](40)` in `def invoke_captured[origins: OriginSet, //,
  callback: def[n: Int](Int) capturing[origins] -> Int]()`, bound to an
  `@parameter` closure that captures a local, runs erased and stops in the
  elaborator with "place keeps symbolic type `def[n: Int](Int)
  capturing[...] -> Int`".
  - The elaborator resolves an indirect call through a capture-free generic
    callable, or a capturing non-generic one passed as a trailing runtime
    argument, but never specializes a capturing generic closure at the
    call's compile-time arguments while passing its environment.
  - `conformance/fixtures/generic_anonymous_callables.mojo` (artifact
    conformance case `generic-anonymous-callables`) and
    `generic_callable_contract_defaults_override_implementation_defaults`
    (`tests/param_callable_value_test.rs`) pin it.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R327 An out-of-range value-pack index fails as an unsupported
  constant**

  Problem: `values[2]` in `def third[*values: Int]()` called as
  `third[1, 2]()` stops with "keeps the parameter constant `values[2]` in
  elaborated MIR", where the pin rejects the instantiation at compile time.
  - The verdict is right; the words are internal.
  - `Specializer::param_constant` (`mono/specializer.rs`) drops the
    out-of-range error `eval_ct` reports as `Unsupported`, so the verifier
    reports the surviving constant instead of the instantiation.
  - Found while landing R318 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Behavioral Divergences From The Pinned Mojo *(recurring — reopens at every nightly re-pin)*

Track: `divergences`.

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
- The `26cfe94f40` re-pin (2026-09-21, Mojo `1.6.0.dev2026092105`) closed the
  one subset change its window forced: a walrus updates and never introduces
  a binding. The window's unimplemented features are the changeset in
  [`docs/mojo-nightly.md`](mojo-nightly.md) and are filed separately; the two
  divergences it found are checkboxes below.

Behavioral divergences from the pinned Mojo, to burn to zero. Every new
divergence lands here with a probe or a `cases.tsv` `mojito-only` /
`output-diff` row, and leaves when its probe promotes to an `assets/ok`
fixture. Each entry's `Ledger name` is the slug `conformance/cases.tsv` uses
for it, and an entry that waits on another names it. Five divergences are
retained on purpose and re-probed rather than fixed; they are listed in
[`docs/non-goals.md`](non-goals.md).

- [ ] **R162 A compile-time evaluation of a keyed function that recurses
  under a runtime `if` prints a value where the pin expands it without
  end**

  Problem: `comptime R = rep[3]()`, where `rep[n: Int]` calls `rep[n - 1]()`
  under a runtime `if n == 0`, prints `6` in Mojito. The pin expands the
  instantiation without end (killed after 60 s, its depth unlimited by
  default).
  - Cause: Mojito's compile-time evaluation runs the erased body with `n`
    reified at run time.
  - Pinned by `conformance/probes/ctfe_plain_keyed_recursion.mojo`.
  - Closes with R7, when the evaluation demands concrete instances and the
    instance budget stops the expansion.
  - Ledger name: `ctfe-plain-keyed-recursion`.
  - The work is R7's, so this entry is taken as-is when R7 lands.
  - Depends on R7, which makes the evaluation demand concrete instances.
  - Model: Opus, Not Planned.

- [ ] **R163 `len` over a runtime pack is accepted in a `comptime for`
  header the pin rejects**

  Problem: `comptime for i in range(len(items))` over a runtime pack
  `*items: *Ts` runs in Mojito and is rejected upstream ("cannot use a
  dynamic value in call argument"). Both accept `items.__len__()` there.
  - Pinned by `conformance/probes/pack_len_comptime_for_header.mojo`.
  - The lever is the builtin `len` over a `VariadicPack` under source
    validation and the elaborator's `len(args)` fold
    (`specialize.rs:generate_def_spec`).
  - Both answer the arity where the pin treats the call as dynamic.
  - Ledger name: `len-over-pack-in-comptime-for-header`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R164 A trait method with a trailing `where` clause is accepted, though
  the pin does not support `where` on trait methods**

  Problem: a trait method declared with a trailing `where` clause (`def
  push[H: Movable](mut self, value: H) where conforms_to(H, Copyable): ...`)
  is accepted by Mojito and rejected upstream ("'where' clauses on trait
  methods are not supported", 2026-09-26).
  - A struct's witness of such a requirement is rejected as not matching
    the signature, with or without the same clause.
  - Pinned by `conformance/probes/where_on_trait_method.mojo`.
  - The lever is the trait-method arm of `check_trait`
    (`checker/traits.rs`), which compiles the clause onto the method's last
    binder.
  - Ledger name: `where-on-trait-method`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R165 A free function returning an owned interior of its argument
  escapes the call-result aliasing rule, so `w = keep(view_x(w))` runs**

  Problem: a free function whose return declares an owned interior of an
  argument is not judged by the call-result aliasing rule
  (`checker/origins/result_alias.rs`). So `w = keep(view_x(w))` runs in
  Mojito and is rejected upstream.
  - `view_x(v: W) ->
    StringSpan[origin_of(v.x)._get_owned_interior["bytes"]]` carries its
    argument's origins unprojected.
  - Cause: only methods record `view_result_interiors`.
  - A free call needs the same side table keyed by the projected parameter.
  - Free-function signatures keep no source return type the call site can
    read, so the plan picks where the parameter projection is recorded.
  - Ledger name: `result-alias-rule-coverage`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R166 Unpack-assigning a call over a view of the assigned local is
  rejected as a conflict the pin does not see**

  Problem: `a, b = pair(a.rstrip())` runs upstream (`ab 1`), while Mojito
  rejects it with "access to 'a' conflicts with live reference
  '$arg_loan_r5'".
  - Cause: the argument's view anchor outlives the call into the unpacking
    store.
  - The anchor's statement-end keep-alive is what every other call argument
    relies on.
  - Shortening it for unpacking needs its fallout checked first.
  - Ledger name: `unpack-assign-call-over-viewed-local`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R167 Moving a `var` parameter into a local collection in a generic
  method is rejected as a use of an uninitialized value**

  Problem: in a generic struct's method, `result.append(value^)` into a
  local `List[Self.T]`, with `value` a `var` parameter of type `Self.T`,
  then `return result^`, runs upstream. Mojito rejects it with "use of
  uninitialized value 'value'".
  - Pinned by `conformance/probes/moved_parameter_into_local_list.mojo`.
  - A parameter whose type may carry loans stands for the caller's loans by
    its own place (`declarations.rs`, the parameter's aggregate origins).
  - So `append`'s transfer effect installs a loan on `value` in `result`,
    and the erased generic body reads `value` after it moved.
  - A field of `self` as the destination installs nothing in the frame
    (`install_call_transfers`).
  - A concrete element type carries no loans.
  - So only the erased body with a local destination rejects.
  - The lever is what a moved source's stand-in place means once the source
    is gone: its loans outlive the move, the place does not.
  - Ledger name: `moved-parameter-into-local-collection`.
  - Depends on R49, the same stand-in place through a sibling call's `mut`
    argument.
  - Model: Opus, Planned.

- [ ] **R168 `String`, `List` and four other bundled stdlib types are not
  trivially movable in Mojito, though they are upstream**

  Problem: `IsTriviallyMovable[String]`, `IsTriviallyMovable[List[Int]]`,
  and the `MaybeUninit` conformances that follow from them are `False` on
  Mojito and `True` upstream.
  - Cause: six bundled stdlib types (`String`, `List`, `Array`, `Dict`,
    `Set`, `Optional`) declare an explicit `__init__(out self, *, deinit
    move: Self)` where upstream relies on the implicit bitwise move.
  - The predicate itself agrees on hand-written structs.
  - Deleting the six move constructors is the fix.
  - It hands every heap-owning move to the compiler-generated path on both
    backends, so the plan checks that path first.
  - Ledger name: `trivially-movable-stdlib-types`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R169 A `ref` binding to a register-passable value is accepted,
  though the pin rejects it for having no memory origin**

  Problem: a `ref` binding to a register-passable value is rejected upstream
  (`value of type 'Int32' doesn't have a memory origin in 'ref' binding`)
  but accepted by Mojito.
  - It covers `ref y = f()` for an `Int`-returning `f`, `ref y = a + 1.0`,
    and a SIMD lane read `ref lane = v[i]`.
  - The lane binding also writes through to the vector, so `lane += 2`
    changes `v`.
  - A memory-backed temporary (`ref x = make_list()`) is accepted by both.
  - The lever is the `StmtKind::RefDecl` arm in
    `crates/mojito-checker/src/checker/statements.rs`, whose
    `materialized_reference_actual` fallback materializes any value.
  - Pinned by `conformance/probes/ref_binding_register_value.mojo`.
  - The lever is named, but the rejection reaches wide fixture fallout, so
    it wants its own pass with the fallout enumerated first.
  - Ledger name: `ref-binding-register-value`.
  - Depends on R54, which wants the same fallback to materialize a literal
    for a `ref` parameter: the two must agree on what it may materialize.
  - Model: Opus, Planned.

- [ ] **R170 Writing a `Tuple` element (`t[0] = 9`) is rejected, though the
  pin's subscript is a mutable place**

  Problem: `t[0] = 9` on a `Tuple` runs upstream and prints `9`, while
  Mojito rejects it with "invalid assignment target: Tuple elements are
  immutable". Upstream's `__getitem__[idx](ref self)` returns `ref [self]`,
  so the subscript is a mutable place.
  - The rejection is a checker rule that predates the reference-returning
    `__getitem_param__` the bundled `std/builtin/tuple.mojo` now declares.
  - So the declaration and the rule disagree about the same hook.
  - Withdrawing it also decides `std/collections/pack_tuple.mojo`, whose
    accessor returns a copied value precisely to keep the write rejected
    (`self_hosted_pack_tuple_preserves_tuple_restrictions`).
  - Pinned by `conformance/probes/tuple_element_write.mojo`.
  - The lever is one rule, but making tuple elements writable changes what
    every tuple place means to ownership analysis.
  - The plan enumerates that fallout first.
  - Ledger name: `tuple-element-write`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R171 Seven corpus fixtures compute different numbers from the pin at
  arithmetic edge cases such as an over-wide shift**

  Problem: seven corpus fixtures compute different numbers from the pin,
  found by the 2026-09-12 stdout sweep and listed in
  [`conformance/assets-mojo-output-diffs.tsv`](../conformance/assets-mojo-output-diffs.tsv)
  under `arithmetic`.
  - The clearest is a shift past the bit width
    (`assets/ok/pliron_straightline.mojo`: `a << 65`, where the pin prints
    7 and Mojito 51102306).
  - The SIMD shift and floor-division fixtures disagree wholesale.
  - Also here: an out-of-range float-to-int cast and `Float64` `**`.
  - `round()`'s half-way case left this list on 2026-09-13, when `round`
    became ties-to-even on both backends.
  - Each case needs the pin's rule established before Mojito's is changed.
  - `docs/native-abi.md` already defines some of them deliberately (wrapping
    overflow), so the plan decides which are bugs and which are recorded
    choices.
  - Ledger name: `native-arithmetic-edge-cases`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R172 Binding a capturing closure to a method's compile-time callable
  parameter retypes the captured value as `None`**

  Problem: a method whose compile-time callable parameter binds a capturing
  closure is rejected — "operator Mul is not defined for Int and None" —
  while the pin runs it. The same closure through a free function is
  `assets/ok/lambda_hof.mojo`.
  - The method call is what breaks it: the program runs once the
    `runner.apply[scale](5)` line goes.
  - So binding the closure to a *method's* parameter is what retypes the
    captured `factor` as `None`.
  - Pinned by `conformance/probes/method_capturing_callable_parameter.mojo`.
  - The native side of this shape is unbuilt behind the checker.
  - Only the direct-call arm promotes a capturing callable argument to a
    runtime parameter (`mono/promote.rs`).
  - So a `MethodCall` carrying one would still reject contextually at
    monomorphization.
  - Ledger name: `method-capturing-callable-parameter`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R173 Two corpus fixtures run their destructors later than the pin
  does**

  Problem: two corpus fixtures run the same destructors later than the pin
  does (`conformance/assets-mojo-output-diffs.tsv`, family `drop-timing`):
  `assets/ok/owned_pointer_api.mojo` and
  `assets/ok/try_region_drop_timing.mojo`.
  - The two `MaybeUninit` fixtures that used to skip destructors outright
    left this list on 2026-09-17, when owning temporaries gained their
    hidden slots.
  - Both remaining cases are orderings rather than omissions.
  - The plan establishes where the pin runs each destructor before Mojito's
    schedule is moved.
  - Ledger name: `destructor-timing-against-the-pin`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R174 A reference into what a `Pointer` field borrows cannot name the
  element sub-origin the pin requires**

  Problem: a method returning a reference into what a `Pointer[T, Self.o]`
  field borrows names the region with the struct's own origin parameter in
  Mojito. The pin distinguishes the element sub-origin
  (`origin_of(o["element"])`) and wants `ref[origin_of(self.src[][i])]`,
  which Mojito's escape check rejects.
  - Cause: no `Origin` variant carries a projected `SelfParam`.
  - Two `assets/origin_ok` fixtures moved for it.
  - Five `Pointer` iteration twins
    (`assets/extensions/ok/pointer_field_reference_yielding_iteration*`,
    `pointer_field_comprehension_borrowed_named_source`,
    `pointer_field_parametric_mut_iterator_read`) joined their `ref`-field
    originals.
  - The fix adds a projected receiver origin to
    `mojito_types::origin::Origin`, which MIR text, verification, and
    substitution all read.
  - Ledger name: `pointee-element-reference-return`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R175 A function-local `comptime` constant must be named in a nested
  `def`'s capture list, where the pin rejects naming it there**

  Problem: a function-local `comptime` constant read from a nested `def` is
  an ordinary immutable local in Mojito, so it must be named in the capture
  list. The pin treats it as a compile-time constant and rejects naming it
  there.
  - Mojito cannot simply stop requiring the capture: the constant has real
    storage in the outer frame and the lifted function has no binding for
    it.
  - The fix makes a folded `comptime` local a constant the lifted body can
    read.
  - That is a lowering change, not a scope-rule change.
  - Ledger name: `local-comptime-capture`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R176 Capturing callables are accepted as runtime arguments, typed
  locals and `mut self` conformers, where the pin takes them only as
  compile-time parameters**

  Problem: four callable-shape leniencies. Mojito passes a capturing lambda
  as a runtime argument, binds one to a local under an explicit
  `capturing[...]` annotation, and accepts a `mut self` `__call__` as a
  `def(...)` conformer.
  - The pin takes a capturing callable only as a compile-time parameter,
    and refuses a lambda even there.
  - The pin types a capturing lambda as a plain `def(...) -> T` that
    converts to nothing.
  - The pin wants a read receiver on `__call__`.
  - Three corpus fixtures moved to `assets/extensions/`.
  - Three more were respelled onto `thin` contracts and `@parameter def`
    arguments.
  - The four share one question — what a capturing callable *value* is —
    and the plan settles that before any rejection.
  - Ledger name: `capturing-lambda-argument`, `capturing-lambda-locals`,
    `owned-capture-closure-locals`, `mut-self-callable-struct`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R177 A struct field pointing into an owned interior of a generic
  origin parameter type-checks, though the pin calls that interior
  never-initialized**

  Problem: a struct field typed `Pointer[T,
  Self.origin._get_owned_interior["tag"]]` over a generic origin parameter
  is accepted by Mojito. The pin parses the projection but calls the
  interior reference never-initialized, so the carrier struct does not
  type-check there at all.
  - Two `assets/ownership_ok` fixtures moved for it.
  - Upstream's owned-interior origins are real; what differs is which
    structs may name one.
  - The plan probes that rule before Mojito narrows.
  - Ledger name: `interior-generation-view-consume`,
    `interior-generation-view-drop`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R178 Mojito's `for` yields the iterable's `Element` where the pin
  yields the iterator's `Iter.Element`**

  Problem: Mojito's `for` yields the *iterable*'s `Element`, so one
  associated type serves a generic signature and the loop. The pin yields
  the *iterator*'s `Iter.Element` and will not convert between the two
  without an identity clause Mojito does not implement.
  - `assets/extensions/ok/iterable_associated_element.mojo` has no `main`.
  - Its `for` over a trait bound that declares only `Element` never
    compiled upstream.
  - Upstream, even a real `Iterable` bound abandons its `AnyType` iterator
    temporary.
  - Ledger name: `iterable-element-identity`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R179 Mojito's stdlib ships algorithm modules and owning-family
  container APIs that upstream does not have**

  Problem: two stdlib surfaces upstream does not have — `std.algorithms`,
  `std.collections.string_dict`, and the owning-family container APIs
  (`deinit_with`, `clear_with`, displacement-returning `insert`).
  - Two corpus fixtures moved to `assets/extensions/` for them.
  - Whether these leave or stay is a stdlib-shape decision, not a
    respelling.
  - Ledger name: `mojito-only-stdlib-algorithms`,
    `owning-family-container-apis`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R180 A conditional partial move joined with a whole move is
  accepted, though one path leaves a hole the pin rejects**

  Problem: a conditional partial move on one branch joined with a whole move
  on the other is accepted. The first path reaches the exit with a hole the
  pin rejects (`field 'p.a' destroyed out of the middle of a value`).
  - Cause: the three-point move lattice joins `a: MaybeMoved` under an
    intact base with a wholly moved base into a state it cannot tell from
    intact-or-wholly-moved.
  - Pinned by `conformance/probes/partial_move_join_imprecision.mojo`.
  - The lever is a fourth lattice point, or a per-node "may hold a hole"
    flag that survives joins.
  - The plan picks one.
  - Ledger name: `partial-move-join-imprecision`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R181 Assigning a call over a `Span` of the assigned `List` is
  rejected as a conflict the pin does not see**

  Problem: `xs = rebuild(Span(xs))` runs upstream (`1`) and is rejected in
  Mojito with "access to 'xs' conflicts with live reference 'xs'".
  - The temporary argument's anchor now ends before the store, as for the
    passing `s = String(StringSpan(s))`.
  - But the assigned `List[Int]` result still records a loan on `xs`, so
    the store conflicts with the new value itself.
  - Pinned by
    `conformance/probes/assign_plain_span_argument_over_list.mojo`.
  - Where the result's loan comes from (MIR `aggregate_borrows` or a
    replayed transfer effect) is not yet known.
  - Ledger name: `assign-plain-span-argument-over-list`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R182 A view struct with a direct `ref` field still widens a
  returned field view to `origin_of(self)`, which the pin rejects**

  Problem: a view struct that stores its source in a direct `ref` field
  still widens a returned field view to a declared `origin_of(self)`.
  - `def view(ref self) -> View[origin_of(self)]` returning `View(source,
    0)` over `ref source = self.items` passes when `View` holds `ref[o]
    List[Int]`.
  - The `Pointer`-field twin rejects with upstream's "cannot implicitly
    convert 'View[origin_of(self.items)]' value to
    'View[origin_of(self)]'"
    (`assets/type_error/return_origin_widening_field_view.mojo`).
  - Why the tail escapes `reconcile_return_origin_tails`
    (`checker/origins/solve.rs`) is not yet known.
  - The likely lever is the tail the `ref`-field constructor binds.
  - Pinned by `assets/extensions/ok/ref_field_view_for_temporary.mojo`,
    `ref_field_view_ref_yield.mojo`, `ref_field_view_method_return.mojo`,
    `ref_field_drain_mut_method.mojo`, and
    `ref_field_chained_view_call.mojo`.
  - The rule already exists; the gap is one constructor path.
  - Ledger name: `ref-field-return-origin-widening`.
  - Depends on R27, which tightens the returned-origin comparison this
    constructor path escapes.
  - Model: Opus, Planned.

- [ ] **R183 `s[0]` on a `String` yields an `Int` where the pin yields a
  character, and two other texts differ from the pin**

  Problem: `s[0]` on a `String` yields a character upstream and an `Int` in
  Mojito (`assets/ok/nominal_string_indexing.mojo`, `h` against `104`).
  - Two smaller text divergences ride along in the same manifest under
    `one-off`.
  - The raised `DictKeyError` renders differently
    (`assets/ok/self_hosted_dict.mojo`).
  - Reflection prints `<unprintable>` upstream where Mojito prints the
    element types (`assets/ok/type_names_applied_elements.mojo`).
  - The subscript's element type is a declaration change in the stdlib's
    `String`.
  - The other two are texts to match.
  - Ledger name: `string-subscript-element`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R184 `Int / Int` is true division into `Float64` in Mojito, where
  the pin truncates back to `Int`**

  Problem: `Int / Int` is true division into `Float64` in Mojito. The pin
  truncates back to `Int` and divides only an `IntLiteral` pair into a
  float.
  - An `output-diff` row, not a rejection, so it is not on the burn-down.
  - The result type of one operator changes, and every fixture and stdlib
    body that divides integers moves with it.
  - The template-fact derivation materializes a folded name's `i / 2` to
    the template's `Float64` (`folded_arithmetic`).
  - Once the template records `Int` there it refuses.
  - But the elaborator's literal fold then divides an `IntLiteral` pair
    into a float where the pin keeps `Int`.
  - Ledger name: `int-true-division`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R185 Infix SIMD comparisons are elementwise at every width, where
  the pin limits `<` and friends to `Scalar` and gives `==` whole-vector
  meaning**

  Problem: Mojito's `<`/`<=`/`>`/`>=` are elementwise at every width and its
  `==`/`!=` compare lane by lane. The pin constrains the strict
  inequalities to `Scalar` and gives `==`/`!=` whole-vector meaning,
  pointing at `SIMD.lt(...)`.
  - Mojito has had `SIMD.lt(...)`, along with `le`/`gt`/`ge`/`eq`/`ne`,
    since 2026-09-13.
  - Six corpus fixtures were respelled onto the methods.
  - Withdrawing the infix spelling is a leniency to remove, with fallout
    across the stdlib's SIMD bodies.
  - Until then only a width-one SIMD conforms to `Equatable` and
    `Comparable`.
  - The pin's vectors conform too, with `==` reducing the lanes.
  - Ledger name: `simd-infix-comparison`.
  - Depends on R143, the scalar comparison methods: once the infix spelling
    goes, `x.ne(y)` is the only ordered comparison left on a scalar, and it
    does not resolve there yet.
  - Model: Fable, Planned.

- [ ] **R186 SIMD and `Int` constructors accept a narrowing element, an
  inferred width and a `FloatLiteral` the pin rejects**

  Problem: three small constructor leniencies.
  - Mojito narrows a SIMD element argument to the lane type, wrapping an
    out-of-range literal and a wider runtime value. The pin wants the
    lane's own scalar.
  - Mojito infers an unbound SIMD width from the argument count. The pin
    wants a written-out width.
  - Mojito truncates a `FloatLiteral` straight to `Int`. The pin wants the
    `Float64` it truncates from.
  - Three leniencies to withdraw in one pass.
  - Ledger name: `simd-element-narrowing`, `simd-inferred-width`,
    `float-literal-to-int`.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R187 `reduce_mul` over `Float32` lanes folds left at lane precision,
  where the pin reduces pairwise**

  Problem: `reduce_mul` over `Float32` lanes folds left at lane precision in
  Mojito and pairwise in the pin (`319256416.0` against `319256448.0` on
  sixteen lanes).
  - `assets/ok/simd_wide_widths.mojo` now reduces an exactly representable
    vector to avoid it.
  - The float-format entry (R159) hides the same rows.
  - The reduction shape changes in the VM and in the
    `llvm.vector.reduce.fmul` lowering together.
  - Ledger name: `float32-reduce-ordering`.
  - Depends on R159, whose float-format difference hides the same rows.
  - Model: Fable, Planned.

- [ ] **R188 `sp[byte=i]` and `s[codepoint=i]` yield a byte value and a
  `Codepoint`, where the pin yields one-byte and one-codepoint views**

  Problem: `sp[byte=i]` reads the byte value in Mojito and returns the
  one-byte view upstream. `s[codepoint=i]` is a `Codepoint` in Mojito and a
  one-codepoint `StringSpan` upstream.
  - The same shape as `string-subscript-element` (R183), on the keyword
    subscripts.
  - Declaration changes in the stdlib's `String`/`StringSpan`.
  - Ledger name: `string-span-byte-index`, `string-codepoint-index`.
  - Depends on R183, the same declaration change on the positional
    subscript.
  - Model: Fable, Planned.

- [ ] **R189 `String.split` returns owned strings where the pin returns
  views, so assigning over the split source runs only in Mojito**

  Problem: `String.split`/`splitlines` return `List[String]` in Mojito and
  owned-interior `StringSlice` views upstream. So `var parts = s.split("
  ")` then `s = String(parts[0])` runs in Mojito and hits upstream's
  call-result aliasing rejection.
  - Pinned by `conformance/probes/split_returns_owned_strings.mojo`.
  - An API shape change with display and iteration fallout across every
    `split` caller.
  - Ledger name: `split-returns-owned-strings`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R190 A contiguous `List` slice is an owned `List` where the pin
  returns a borrowing `Span`**

  Problem: a contiguous `List` slice is an owned `List` in Mojito and a
  borrowing `Span` upstream, so only Mojito returns one from a `->
  List[Int]` function.
  - The call-result aliasing rule rides on it: upstream rejects `xs =
    rebuild(xs[0:1])` because the slice views `xs`'s owned elements.
  - Mojito's copy borrows nothing and runs.
  - Pinned by `conformance/probes/list_slice_copies.mojo`.
  - The return type of `List.__getitem__(ContiguousSlice)` changes, and
    every caller that owns the result moves with it.
  - Ledger name: `contiguous-slice-result`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R191 `Int` conforms to `Floatable` and `len` resolves from a bare
  `__len__`, though the pin allows neither**

  Problem: Mojito conforms `Int` and an integer literal to `Floatable`.
  Upstream conforms neither, so a `Floatable`-bounded helper takes only a
  float there.
  - Mojito also resolves `len(x)` from a bare `__len__` where the pin wants
    a declared `Sized` conformance.
  - `assets/ok/dunder_index.mojo` and `assets/ok/self_hosted_vec.mojo` now
    declare it.
  - Two conformance leniencies to withdraw.
  - Ledger name: `int-is-floatable`.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R192 Mojito keeps split slice kinds, unknown struct decorators and
  the `ImplicitlyDeletable` alias, which the pin has dropped or never had**

  Problem: three spellings the pin has dropped or never had.
  - Mojito splits upstream's single `Slice` into
    `ContiguousSlice`/`StridedSlice` and overloads subscripts on the kind.
  - Mojito ignores an unmodeled struct decorator where the pin rejects an
    unknown one (`@value` is now unknown there).
  - Mojito still normalizes `ImplicitlyDeletable` to `Deinitable`, which
    the pin has removed.
  - Each is a name or a type to withdraw, with stdlib and fixture fallout.
  - Ledger name: `slice-descriptor-kinds`, `unmodeled-struct-decorator`,
    `implicitly-deletable-alias`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R193 Two live interior references from `ref self` accessors on one
  owner are accepted, though the pin invalidates the first**

  Problem: `print(t.value_at(0), t.value_at(1))`, two `ref self` accessors
  returning an element's interior on one `var` owner, runs in Mojito and is
  rejected upstream ("use of invalidated interior reference
  't.entries["element"]'").
  - Pinned by
    `conformance/probes/interior_reference_two_live_accessor_results.mojo`.
  - The pin takes the second call as a mutable borrow that invalidates the
    first result.
  - A `List` subscript pair (`l[0], l[1]`) passes both.
  - Mojito records no conflict between the two results.
  - The lever is where a `ref self` call's interior reference meets a later
    mutable borrow of the same owner.
  - Found while deriving element-field reference results; not root-caused.
  - Ledger name: `two-live-accessor-interior-references`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R194 A free `def` parameter typed `SIMD[_, _]` is rejected, though
  the pin accepts it**

  Problem: `def bits(value: SIMD[_, _])` runs upstream, while Mojito rejects
  the parameter with "not a valid SIMD element type: a non-DType argument".
  - Pinned by `conformance/probes/simd_wildcard_to_bits_default.mojo`.
  - The `SIMD[_, _]` desugar (`synth.rs:desugar_simd_keyed_methods`) runs
    over struct methods only, where the `Hasher` protocol needs it.
  - A free `def` keeps the wildcard spelling and resolves it as an
    annotation.
  - Found while probing the defaulted `to_bits()`.
  - Ledger name: `wildcard-vector-parameter-on-a-def`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R195 A `Pointer` type argument spelling a local's uninitialized
  interior origin is accepted, though the pin rejects it as a use**

  Problem:
  `unsafe_alloc[Pointer[Int, origin_of(a)._get_owned_interior["element"]]](1)`
  over a local `Array` runs in Mojito and is rejected upstream ("use of a
  never-initialized interior reference 'a["element"]'").
  - Pinned by
    `conformance/probes/pointer_type_argument_uninitialized_interior.mojo`.
  - The pin treats the spelled interior origin as a use of the local's
    interior, which nothing has initialized.
  - Mojito resolves the spelling to a place origin and checks no
    initialization of the interior.
  - Found while baking an interior-projected `Pointer` type argument into
    its clone (2026-09-28).
  - Ledger name: `pointer-type-argument-uninitialized-interior`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R245 A compile-time `break` or `continue` in a `comptime for` the
  elaborator unrolls in the AST is rejected**

  Problem: the pin honors a `break` or `continue` in every `comptime for`;
  Mojito honors one only in a loop its template serves (a `range` loop of a
  generic `def`), and rejects it with "a 'break' or 'continue' in a comptime
  for over 'i' the elaborator unrolls" everywhere else.
  - A loop in a non-generic `def` or a method, over a compile-time
    collection or a pack, or with a local `comptime` binding in its body is
    unrolled above MIR, where a spliced copy's `break` would leave the
    enclosing loop instead (`comptime/elab.rs::unroll_comptime_for`).
  - Closes when every `comptime for` is served by its template (R246, R2)
    and a method's compile-time control flow is (stage P3e of
    `docs/parametric-mir-plan.md`).
  - Ledger name: `comptime-for-break-in-unrolled-loop`.
  - Depends on R246.
  - Model: Opus, Not Planned.

- [ ] **R273 A runtime read of a symbolic `field_names()` binding in a
  generic body that is never called is accepted**

  Problem: `print(names[i])` over `comptime names = reflect[T].field_names()`
  in a generic `def` that no call instantiates runs in Mojito, while the pin
  rejects the template ("cannot materialize comptime value of type
  'Array[StringSpan[ImmStaticOrigin], ...]'").
  - An instantiated body is rejected as the pin does: the elaborator's
    crossing pass sees the closed list per instance
    (`assets/type_error/comptime_field_names_runtime_use.mojo`).
  - Source validation types `names[i]` and `len(names)` through
    `checker/reflection.rs::infer_reflection` without knowing whether the
    position is a runtime one; a compile-time binding or `materialize`
    operand reaches the same path.
  - Pinned by `conformance/probes/reflected_names_runtime_read_uncalled.mojo`.
  - Ledger name: `reflected-names-runtime-read-uncalled`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Mojito-Specific Shortcuts To Move Toward Mojo's Shape

Track: `mojo-shape`.

Parts of Mojito's stdlib lean on the Rust runtime where upstream is pure Mojo,
or take a Mojito-specific shortcut where upstream has its own shape. Each entry
below is a candidate port toward Mojo's shape, and a port is preferred over any
new bridge (2026-09-07 direction). The ports are independent of each other, and
an entry that waits on another names it. Four runtime services are
deliberately not on this list; they are in [`docs/non-goals.md`](non-goals.md).

- [ ] **R306 A scalar `range(...)` is typed by a checker rule where the
  pin declares infer-only overloads**

  Problem: `range(Int32(4))` is typed by `Checker::infer_scalar_range`
  once overload selection fails, and the elaborator rewrites the call into
  the range struct's construction, where the pin's `std.builtin.range`
  declares `def range[dtype: DType, //](end: Scalar[dtype]) ->
  _ZeroStartingRange[dtype]` and its two- and three-argument siblings.
  - Mojito's float range is a separate `_FloatStridedRange`; the pin's
    `_StridedRange[dtype, forward]` takes the float path under `comptime if
    Self.dtype.is_floating_point()`.
  - The overloads reject a float or `Bool` lane with `comptime assert`,
    which Mojito does not parse (R74).
  - With the overloads in source, `infer_scalar_range`,
    `scalar_range_requests`, the elaborator's range rewrite, and the
    checker's discovery-round range shortcuts (`scalar_range_parts`) go.
  - Depends on R74.
  - Model: Opus, Planned.

- [ ] **R321 A `comptime for` iterates by a built-in rule, not by its
  iterable's iterator**

  Problem: Mojito unrolls a `comptime for` over a `range`, a compile-time
  list, set, or dictionary, or a pack by a rule of its own
  (`CtValue::comptime_iteration_elements`, `ComptimeSequence`), and rejects
  any other iterable with "'comptime for' iterates a range, a compile-time
  list, or a pack", where the pin runs a user `Iterator & Copyable`
  (`comptime for x in Count(3)` prints `2`, `1`, `0`).
  - Upstream's parser desugars every `comptime for` to `seq.__iter__()` in
    the parameter domain, and the elaborator steps it through the stdlib
    stubs `paramfor_has_next`, `paramfor_next_iter`, and
    `paramfor_next_value` (`std/builtin/_stubs.mojo`).
  - Following it moves the MIR header's binder from the element to the
    iterator, the element bound as `paramfor_next_value(it)`, with each
    step a compile-time application the elaborator runs.
  - Depends on R246.
  - Model: Fable, Not Planned.

- [ ] **R159 `Float64` and `Float32` print different text from the pin
  because Mojito formats floats in Rust rather than in Mojo**

  Problem: upstream formats `Float64` in Mojo (Dragonbox in `format_float`),
  while Mojito formats it in Rust: the float arm of the VM's `Display for
  Value` and the native `mjrt_fmt_f64`.
  - Five corpus fixtures print different text on the two compilers because
    of it (`conformance/assets-mojo-output-diffs.tsv`, family
    `float-format`).
  - The pin writes an exponent sign: `1e+23`, where Mojito writes `1e23`.
  - The pin renders a `Float32` at its own precision: `0.1`, where Mojito
    writes `0.10000000149011612`.
  - The lever: transliterate a permissively licensed Rust Dragonbox (MIT or
    Apache-2.0 — third-party crates are allowed, see `AGENTS.md`) into Mojo
    rather than deriving the algorithm.
  - The plan picks the source and pins the shortest-round-trip cases.
  - Only a from-scratch derivation would want Fable.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R160 `DType` is a compiler builtin in Mojito where upstream's is a
  stdlib struct over a one-byte code**

  Problem: Mojito's `DType` is `Ty::Dtype`, with its `is_*` queries and
  display built into the VM and the native lowering. Upstream's `DType` is a
  stdlib struct over a one-byte code.
  - A bundled `struct DType` needs struct-valued associated `comptime`
    members, which `eval_associated_ct` rejects today.
  - It also needs runtime reads of `StructName.NAME`.
  - It also needs struct value parameters on defs.
  - It also needs a bridge from a frozen struct value to `Dtype` at every
    hard-wired `DType` site.
  - Several checker and elaborator capabilities land before the struct can
    replace the builtin.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R324 `reflect[T]` is a compiler builtin where upstream's is a
  stdlib struct over parameter attributes**

  Problem: Mojito answers `reflect[T]` queries by built-in rules in the
  checker (`checker/reflection.rs`), the crossing pass, `comptime/eval.rs`,
  and the elaborator (`mono/symbolic.rs`), where upstream's `Reflected[T]`
  (`std/reflection/reflect.mojo`) is a stdlib struct whose methods return
  parameter attributes its elaborator folds.
  - `field_count()` is `_field_types_of[T]().length` over
    `#kgen.struct_field_types`, `is_struct()` is `#kgen.is_struct_type`, and
    `field_index[name]()` is `#kgen.struct_field_index_by_name`.
  - The shared policy is one function today (`ReflectQuery::answer`,
    `param_expr.rs`), so the answers agree; the shape does not.
  - Closing it means a bundled `Reflected` over intrinsics that lower to
    `ParamKind::Reflect`, with the built-in rules deleted.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R276 `SIMD` writes its text in the host where upstream's writes it in
  Mojo**

  Problem: `print` and `repr` of a `SIMD` value are built into the VM
  (`format_value`, `simd_repr` in `backend/vm/dispatch.rs`) and the native
  lowering (`lower_repr_builtin`, `print_simd`). Upstream's `SIMD` is a
  stdlib struct whose `write_to` and `write_repr_to` are Mojo, choosing the
  scalar alias through `_scalar_repr_alias`.
  - The texts match the pin; the shape does not.
  - A bundled `struct SIMD` needs everything R160 needs for `DType`, plus
    `Ty::Simd`'s slots served by a nominal struct.
  - Depends on R160.
  - Model: Fable, Not Planned.

- [ ] **R161 `Tuple`'s default initializer stores into private storage where
  upstream marks `self` initialized and writes each element through a
  pointer**

  Problem: Mojito's `Tuple` default initializer stores each element to the
  private storage (`self.storage[i] = Self.Ts[i]()`). Upstream marks `self`
  initialized and writes each element through a pointer
  (`Pointer(to=self[i]).unsafe_write({})`).
  - Mojito has no `mark_initialized`.
  - Mojito has no pointer to an uninitialized element.
  - Mojito has no contextual `{}` construction.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R36 `Layout` carries its alignment as a runtime field**

  Problem: `Layout[Int](count=1, alignment=16)` is a runtime keyword argument
  in Mojito, where the `26cfe94f40` pin takes alignment as a keyword-only
  compile-time parameter and reports "unexpected keyword argument
  'alignment'".
  - Upstream spells it `Layout[Int32, alignment = .of_bytes[64]()](count=8)`,
    builds the value with a new `Alignment` type (`Alignment.of[T]()` for a
    natural alignment), and gives `Allocation` and `ManagedAllocation` the
    same parameter while `ThinAllocation` deliberately has none.
  - Only compile-time alignments are supported upstream, so the parameter is
    the whole surface; nothing needs a runtime alignment field.
  - `assets/ok/layout_allocation.mojo` is the fixture the pin rejects
    (`conformance/assets-mojo-rejects.tsv`, family `layout-alignment`).
  - The lever is `stdlib/std/memory/alloc.mojo` (`Layout`, `alloc`,
    `_RawAlloc`) plus the VM's reservation check.
  - Moving a field to a parameter changes every `Layout` value's type, so the
    plan must first find what depends on the field.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R86 `String` always owns a heap buffer, where upstream's has three
  representations**

  Problem: `String(literal)` allocates and copies the literal's bytes, while
  upstream points at the static bytes and copies only on the first mutation.
  - Upstream packs a static-constant, an inline (up to 23 bytes), and a
    reference-counted heap form into the same 24 bytes, flagged in
    `_capacity_or_data`; Mojito's `{data, size, cap}` has only the heap form.
  - No program output differs; allocation counts and `capacity()` do.
  - Port the static form first: every mutator, `__del__`, copy, and move
    must respect a non-owning flag, on both backends.
  - Depends on nothing.
  - Model: Opus, Planned.

### Grow The CPU Standard Library *(demand-first)*

Track: `stdlib`.

The standard-library surface grows on demand toward the audited head: the
tuple, slice, optional/variant, and String surfaces, and the filesystem and
I/O residues left behind the landed files, streams, paths, and tempfile stage.
`docs/features.md` records what lands. Each entry below is a conscious,
recorded limit, listed in impact order: soundness of the executable oracle
first, then everyday spellings that reject today, then parity details. A
residue found inside a task moves to the task that owns its fix.

- [ ] **R196 A pointer-backed result of VM compile-time evaluation cannot
  cross back into the program**

  Problem: a VM-evaluated compile-time expression whose result is
  pointer-backed (`comptime C = M.copy()`, a bare `Optional`, a `String`)
  fails with `cannot cross back from VM CTFE`, where upstream binds it.
  - The freezable results today are scalars, Bool, String, tuples, fieldwise
    structs, and displays.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R197 Compile-time Dict/Set key identity ignores a user struct's
  non-fieldwise `__eq__`**

  Problem: compile-time Dict/Set key identity is structural `CtValue`
  equality, which diverges from upstream for a user-struct key with a
  non-fieldwise `__eq__`.
  - Structural equality is exact for every prelude key type.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R198 The typing probe re-checks the CTFE subprogram for every
  VM-bound expression**

  Problem: the typing probe checks the CTFE subprogram once more per
  VM-bound expression, a compile-time cost upstream does not pay.
  - No lever is named.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R199 A string element of a compile-time tuple indexed under
  `comptime for` breaks inside a list display**

  Problem: with `comptime t = (1, "s")` and `comptime for i in range(2)`,
  `first([t[i], t[i]], t[i])` fails MIR verification with `register r19 has
  no checked type`.
  - `var v = t[i]` followed by `first([v, v], v)` fails with `binding of
    StringLiteral to a slot of type String`.
  - The `Int`/`Bool` twin runs.
  - Cause: the materialized element keeps its literal type where the
    display's element type has already materialized `String`.
  - Two `var` declarations across unrolled iterations also collide (`'v' is
    already declared in this scope`) unless each body opens a block. That
    collision is R40's to fix.
  - Depends on R40, whose `comptime for` scoping fix lets the `var v` repro
    run without a block per body.
  - Model: Opus, Not Planned.

- [ ] **R200 A generic instantiated at `StringLiteral` by a literal argument
  keeps the erased path**

  Problem: a generic instantiated at `T = StringLiteral` by a literal
  argument runs erased, since `instance_method_clone_name` mints no clone
  for it.
  - Displays and `StringDict` keys now materialize `String`, so they no
    longer reach it.
  - Unchecked: whether `StringDict.__getitem__` can now return a reference
    instead of its `Copyable`-guarded copy.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R201 A call inside an unstamped bundled body on a bundled struct's
  generic method keeps the erased path**

  Problem: such a call mints no instance clone and runs erased.
  - Requests are admitted from user code, clone bodies, variadic specs,
    instances, and user structs only.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R202 An instance clone whose walk cannot resolve an application
  falls back to the erased path, or rejects a program the pin accepts**

  Problem: an instance clone whose walk cannot resolve an application (a
  variadic template over a nested public `Tuple` argument) is dropped to the
  erased path rather than failing the program.
  - When such a method can reach a compile-time-keyed stub, an instance the
    elaborator queued but could not clone rejects the program instead —
    rejecting one the pin accepts.
  - An instance the checker records no instantiation for at all (a type
    parameter inferred as the compile-time `StringLiteral`, `Box("a").f()`)
    is not queued, so it keeps the erased path and aborts at run time.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R203 Bundled templates build and copy their instances through the
  erased constructors**

  Problem: a `List`/`Dict`/`Optional` instance builds and copies through the
  template's `__init__`/`__copyinit__`/`__moveinit__` on the erased path, so
  a `comptime if Self.T` there would not fold.
  - User structs clone their constructors.
  - Every struct's `__deinit__` clone is reached.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R204 An erased body dispatches by runtime name where no checked
  static type reaches it**

  Problem: a compile-time-keyed method reached through runtime-name dispatch
  aborts rather than being rejected.
  - Sites: an operator or protocol dunder called from another erased body.
  - Sites: a value whose static type is a bare `Ty::Param`.
  - Sites: an instance reached only from bundled code.
  - Sites: CTFE, which runs before any clone exists.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R205 Instance clones are minted per whole instance without
  reachability pruning, inflating compile time**

  Problem: clones are minted per whole instance and re-checked each
  discovery round.
  - `benchmarks/compile/stdlib_heavy` is about 2.2x its pre-clone baseline
    in release (`docs/performance.md`).
  - The repr methods added 2026-09-05 cost about 5% in debug across the
    compile benchmarks.
  - Lever: reachability-pruned minting.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R206 Type-pack calls inside a nested `def` and whole-pack-forwarded
  calls keep the syntactic element-typing path**

  Problem: `take(b, 1)` over a local `b` rejects with `a heterogeneous pack
  specialization needs an expression whose type is statically evident before
  checking`.
  - Top-level calls consult the checker's instantiation.
  - Three of the four pieces exist for ordinary nested generics: an
    unresolved template survives `replace_templates` for the discovery
    check.
  - `def_specialization_requests` harvests a nested callee.
  - `NestedMono::scan_expression` consults that request.
  - Remaining: the checker accepting a nested variadic shell abstractly.
  - Remaining: softening the pack diagnostics. Only an arity failure defers
    today, and a pack failure is `PackBound` or `NotComptime`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R207 A user variadic struct application as a pack element keeps the
  fixed-arity diagnostic**

  Problem: `Tuple[TypeNames[Int]]` rejects with the fixed-arity diagnostic.
  - Its erased shell has no sound nominal form.
  - The public `Tuple` is the one compiler-known template whose `*Ts`
    absorbs every argument.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R208 A public Tuple as an explicit type argument does not conform
  to `Deinitable`**

  Problem: `make[T: Defaultable & Deinitable]()` over `Tuple[Int, Bool]`
  reports the bound failure.
  - The inferred shape runs.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R209 `Tuple.reverse` and `Tuple.concat` are typed in Rust instead
  of declared in Mojo**

  Problem: both are typed in Rust from the element list
  (`checker/method_calls/builtin_types.rs`), not declared in
  `std/builtin/tuple.mojo`, while upstream writes both in Mojo over
  `Self.Ts.reverse()` and `TypeList._concat`.
  - The nominal declaration answers a Tuple method call first, and this
    surface serves only what it does not declare.
  - Mojito has no spelling for the upstream pack operations, so porting
    waits on type-level pack algebra.
  - Pinned by
    `checker_test::accepts_tuple_constructors_and_structural_operations`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R210 A lambda parameter annotated `String` reports an unknown
  type**

  Problem: `lambda (s: String): print(s)` reports `unknown type 'String'`,
  while a nested `def` with the same parameter runs.
  - Likely cause: the lambda's hidden `def` misses the prelude qualification
    the other annotations get.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R211 A list display passed to an explicitly applied constructor
  takes no context from the parameter type**

  Problem: at runtime `Dict[String, Int](["a"], [1], None)` takes no context
  from the parameterized `List[Self.K]` parameter, materializes the displays
  as `Array`, and matches no overload.
  - Workaround: spell the lists (`List[String]("a",
    __list_literal__=None)`), as a compile-time value's materialization
    does.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R212 A struct's own value parameter as a SIMD width in a signature
  is rejected as a type**

  Problem: `def zeros(self) -> SIMD[DType.int64, Self.length]` reports `SIMD
  width must be a positive power of two, got a type`, where upstream binds
  the parameter.
  - Cause: the width slot needs a concrete comptime `Int`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R213 String literals have no methods**

  Problem: `"abc".byte_length()` rejects.
  - Workaround: convert a literal-typed value through `String(...)` first.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R214 An annotated `Optional[T]` local initialized from a bare
  struct construction fails MIR verification**

  Problem: `var o: Optional[P] = P(4)` fails MIR verification with an
  untyped register for the constructor call.
  - Workaround: spell the wrapper (`Optional[P](P(4))`, `Optional(P(4))`).
  - A literal payload (`var n: Optional[Int] = 6`) converts.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R216 A string literal does not bind a trait-bounded type parameter
  that `String` satisfies**

  Problem: `isdir("/tmp")` reports `'StringLiteral' ... does not conform to
  trait 'PathLike'`.
  - Workaround: spell `String("/tmp")`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R217 Struct-level `comptime NAME = Self(n)` constants do not
  fold**

  Problem: upstream's `ErrNo.ENOENT` shape does not fold.
  - Cause: the associated-constant evaluator handles prefix, infix, tuple,
    and list expressions only.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R218 `@fieldwise_init` beside a hand-written `__init__` is
  rejected**

  Problem: a struct carrying both `@fieldwise_init` and a hand-written
  `__init__` rejects.
  - Workaround: spell every constructor by hand.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R219 A bound-generic def whose template body calls itself with a
  concrete argument reports its own specialization undefined**

  Problem: `makedirs(head, exist_ok=...)` inside `makedirs[PathLike]` mints
  its own specialization while checking and reports it undefined.
  - Workaround: recurse through a non-generic helper.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R220 Nullary construction of a sized-scalar array is reported
  unsupported**

  Problem: `Array[Int8, 1024]()` reports the scalar's `Defaultable`
  construction unsupported.
  - Workaround: spell `Array[Int8, 1024](fill=0)`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R221 `Pointer.unsafe_bitcast[U]()` leaves its result at the old
  element type**

  Problem: `Pointer.unsafe_bitcast[U]()` is not typed: an origin-cast-style
  forwarding leaves the MIR register at the old element type.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R222 `CStringSlice` views `Byte` elements instead of `c_char`**

  Problem: upstream's `CStringSlice` views `c_char` elements, Mojito's views
  `Byte`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R223 `std.builtin.rebind.downcast` is missing, so `Dict`/`Set`
  views are guarded by `Copyable` instead**

  Problem: with no `downcast`, `Dict`/`Set` guard `keys`, `values`, `items`,
  and `__iter__` with `where conforms_to(K, Copyable)` instead of laundering
  the parameter.
  - `Dict.keys` needs copyable values as well as keys, since the key view
    wraps the entry view.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R224 Nested struct names print unqualified where upstream keeps the
  module path**

  Problem: `_unqualified_type_name` spells nested structs unqualified where
  upstream keeps a non-prelude struct's module path
  (`Optional[std.collections.dict.Dict[...]]`, `List[up.Flag[True]]`).
  - `List`, `Optional`, `String`, and `SIMD` stay bare.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R225 Grapheme segmentation covers only a UAX #29 essentials
  subset**

  Problem: segmentation is the documented UAX #29 essentials subset, with
  hand-maintained Control/Extend/SpacingMark ranges and no
  Extended_Pictographic or Prepend data.
  - Reverse iteration re-scans forward from the nearest CR/LF/Control
    boundary.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R226 Public `stat` / `lstat` / `stat_result` are not ported**

  Problem: the filesystem stage left public `stat`, `lstat`, and
  `stat_result` unported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R227 `realpath` is not ported**

  Problem: the filesystem stage left `realpath` unported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R228 `symlink` / `link` / `chdir` are not ported**

  Problem: the filesystem stage left `symlink`, `link`, and `chdir`
  unported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R229 `isatty` is not ported**

  Problem: the I/O stage left `isatty` (`FileDescriptor.isatty` / `fchdir`)
  unported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R230 `OptionalPointer` is missing**

  Problem: upstream's `OptionalPointer` is not ported, so null tests spell
  `Int(ptr) == 0`.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R231 `ErrNo`'s named constants are missing**

  Problem: `ErrNo` has none of upstream's named constants (`ErrNo.ENOENT`).
  - A small port.
  - Depends on R217, which folds the struct-level `comptime NAME = Self(n)`
    shape the constants use.
  - Model: Opus, Not Planned.

- [ ] **R232 `~user` path expansion is not supported**

  Problem: `~user` expansion (`getpwnam`) is not ported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R233 The VM has no per-process environment overlay**

  Problem: the VM lacks a per-process environment overlay, while native
  `setenv` writes the real environment.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R234 `NamedTemporaryFile` is not ported**

  Problem: the tempfile stage left `NamedTemporaryFile` unported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R235 `FileHandle.read` cannot read into a typed span**

  Problem: `FileHandle.read` into a typed `Span[Scalar[dtype], origin]` is
  not ported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R236 `Path.stat` / `lstat` / `_dir_of_current_file` are not
  ported**

  Problem: the paths stage left `Path.stat`, `Path.lstat`, and
  `_dir_of_current_file` unported.
  - A small port.
  - Depends on R226, which ports the `stat` / `lstat` / `stat_result` they
    wrap.
  - Model: Opus, Not Planned.

- [ ] **R237 `KeyElement` is missing**

  Problem: upstream's `KeyElement` is not ported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R238 `_get_random_name` reads `/dev/urandom` instead of using
  `std.random`**

  Problem: `_get_random_name` reads `/dev/urandom` today, where upstream
  builds it over `std.random`.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R239 There is no generic `__exit__[E]`**

  Problem: the context-manager surface lacks upstream's generic
  `__exit__[E]`.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R240 An overloaded constructor spelled with the `StringSlice` or
  `Byte` alias mangles a different key than the call selects**

  Problem: an overloaded constructor spelled with the `StringSlice` or
  `Byte` alias mangles a different key than the `StringSpan` / `UInt8` the
  call selects.
  - Workaround: the ports spell the canonical names.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R241 A temporary view used as a method argument or
  augmented-assignment operand fails in the VM**

  Problem: `s.take(String("x").as_bytes())` is a VM "reference receiver must
  be a place" rejection.
  - As an augmented-assignment operand (`p /= StringSpan(s)`) it is a VM
    use-after-free.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R242 `Span[mut=True, T, _]` fails origin inference**

  Problem: a parameter typed `Span[mut=True, T, _]` fails origin inference.
  - Workaround: spell an `[origin: Origin[mut=True]]` binder.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R243 `range` is invisible in `std.string`**

  Problem: code in `std.string` cannot see `range`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R244 A module loaded while the prelude bootstraps must import
  `String` explicitly, and the graph slows Hello World**

  Problem: a module loaded while the prelude bootstraps (`std.io` and the
  whole `std.os` graph now) must import `String` explicitly.
  - That graph costs Hello World about a second of debug compile time
    (`docs/performance.md`).
  - No lever is named.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R143 Scalars have no comparison methods**

  Problem: `x.ne(y)` on a `Float64` (or any width-1 scalar) is rejected with
  `type 'Float64' has no method 'ne'`, though the pin accepts it.
  - Upstream's scalars are width-1 `SIMD`, so `lt`/`le`/`gt`/`ge`/`eq`/`ne`
    exist on them too. Mojito resolves those methods only on a multi-lane
    `Ty::Simd` receiver (`crates/mojito-checker/src/checker/method_calls/simd_receivers.rs`).
  - The methods must keep the multi-lane semantics: the pin's scalar
    `s.ne(s)` is `False` for a NaN (ordered), while infix `s != s` is `True`.
  - Not a wrong answer, only a missing spelling. Until then, `x < y or x > y`
    is the ordered `ne`; infix `!=` answers `True` for a NaN.
  - The `simd-infix-comparison` divergence waits on it: withdrawing the
    infix spelling leaves `x.ne(y)` as the only ordered comparison on a
    scalar.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R144 Time, random, and testing slices**

  Goal: deterministic testable cores, with host-dependent behavior behind
  runtime services.
  - Depends on nothing.
  - Model: Fable, Planned.

### Packaging, Artifacts, And Developer Tooling

Track: `tooling`.

- [ ] **R145 The corpus sweeps no longer run in the overnight gate**

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
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R146 Naming the bundled stdlib with `-I` breaks every program**

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
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R147 Compile-time performance**

  Problem: Hello World is 1.5 s release / 7.9 s debug on the reference
  machine (`docs/performance.md`), and the first checker pass over the
  prelude is now most of it.
  - A body whose inputs are unchanged since the previous checker pass takes
    that pass's facts (`checker/body_carry.rs`), so the later passes of a
    compilation are cheap. The first pass still infers every prelude body.
  - `checked_var_types` scans the whole expression table per variable, and
    `explicit_destroy` re-derives deinitability per struct per pass.
  - Only then cache the elaborated/checked stdlib across processes.
  - R148, R149, and R150 are named costs under the same goal and land in
    any order.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R148 A carried checker pass still copies every fact it keeps**

  Problem: a pass that carries nearly every body still costs about a third
  of an inferring pass (`check_program.bodies` under `--timings`).
  - The carry clones each logged entry into the fresh checker's stores and
    hashes every site's syntax, once per pass (`body_carry.rs:carry_body`,
    `def_syntax_hash`).
  - Moving the previous pass's stores into the fresh checker and removing
    the entries of the bodies it infers would replace the copies.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R149 A clone appended before older clones loses its carry record**

  Problem: a discovery round that inserts a new clone ahead of existing
  ones in the elaborated tree renumbers the older clones' duplicate syntax
  identities, so their records no longer match and they are inferred again.
  - `--timings` notes show them as `body_facts.carry_refused … its syntax
    changed` and `no record in the previous pass` (about a hundred per
    round for Hello World).
  - The final re-key (`ast.rs:rekey_syntax`) numbers replacement identities
    in traversal order. Keying a clone's occurrences by the template
    identity and copy index, as the template mechanism does, would keep
    them stable.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R150 Request identity names binding identities through origins**

  Problem: a tuple, t-string, struct-instance, or method request whose type
  carries an origin rooted at a binding compares by that binding's numeric
  identity, so two rounds that number a body differently see two requests.
  - Carry-over keeps a re-inferred body inside the identity range it had
    (`scopes.rs:reserve_owners`), which is what keeps the request set stable
    today.
  - The elaborator erases origins when it names the clone
    (`mono.rs`), so the requests could be compared with origins erased.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R151 Feature and target options**

  Goal: checked CLI/build configuration recorded in artifacts and
  diagnostics.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R152 Compiled package artifacts**

  Goal: a versioned `.mojoc` representation (modules stay non-first-class).
  - Per-directory resolution order: source package, `.mojoc`, source module,
    legacy `.mojopkg`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R153 Debugging metadata and inspection**

  Goal: stack/source diagnostics, MIR inspection, and debugger-oriented
  value rendering.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R154 Testing tools**

  Goal: Mojito-native assertions, expected-error tests, and
  differential-harness integration.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R155 Seven `erased_vm` trials compare random temporary paths**

  Problem: the corpus binary's `erased_vm` trials of `tempfile_mkdtemp`,
  `with_statement`, `with_multiple`, `path_operations`, `os_dir_operations`,
  `file_handle_roundtrip`, and `file_descriptor_stdout_order` fail, because a
  caught `raise` event carries a random temporary name and the two runs draw
  different ones.
  - The outputs agree, and so do the events apart from the name: "raise
    unable to stat '/tmp/xpgslhxv'" against "'/tmp/d4y7nmno'".
  - The lifecycle log records a raised error's text, and `std.tempfile`
    probes a fresh random name per run.
  - Found on `da941aee` while the parity group was run for the struct-method
    templates (2026-10-01). It fails there too.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R156 Three `comptime_test` callable-argument tests stop in
  elaboration**

  Problem: `specialization_retains_defaulted_thin_callable_arguments`,
  `nested_specialization_retains_capturing_callable_arguments`, and
  `callable_contract_binder_shadows_the_enclosing_binder` fail with an
  elaboration error since the VM runs concrete MIR.
  - The errors: "monomorphization cannot resolve parameter `callback`",
    "binding call to `main$add` during monomorphization: Missing(\"value\")",
    and a parameter type that "keeps symbolic type `def[T: Copyable &
    Deinitable](T) thin -> T`".
  - Each program ran on the erased VM. Whether the elaborator or the test's
    expectation is wrong is not yet known.
  - Found on `da941aee` while `comptime_test` was run for the struct-method
    templates (2026-10-01).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R157 Distribution reproducibility gate** *(last)*

  Goal: the release check rebuilds, tests, documents, and reproduces
  conformance from the crates.io archive alone.
  - It stays last whatever else moves.
  - Depends on every other entry in this track.
  - Model: Fable, Planned.

### Code Organization Follow-Ups *(behavior-preserving)*

Track: `code-org`.

The module splits (`docs/symbol-map.md`) removed every file over 3,000
lines. `checker/template_facts.rs` and its submodules stay frozen: their
line count only goes down, as the `pmir` track deletes the mechanism stage by
stage. The rest needs semantic extraction, not line moves.

- [ ] **R158 Shrink the 2 kloc band** *(standing)*

  Split these further only along a cohesive seam, while touching them:
  - `checker/traits.rs` (2,629), `mir/lower_stmt.rs` (2,595),
    `checker/inference.rs` (2,520), `checker/statements.rs` (2,471),
    `ast.rs` (2,464), `mir.rs` (2,426), `checker/type_resolution.rs`
    (2,395), `runtime.rs` (2,235), `checker.rs` (2,232),
    `comptime/rewrite.rs` (2,179), `checker/declarations.rs` (2,118),
    `mir/text/write.rs` (2,116), `backend/vm/exec.rs` (2,085).
  - Depends on nothing. Split one of these only while touching it for
    another task.

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
  delete it here (no other entry moves or changes) and record the outcome in `docs/features.md`,
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
- Every entry is `- [ ] **R<n> Title**`, with `R<n>` reserved by
  `scripts/roadmap.py new-id`. Never renumber an entry, never reuse an ID,
  and never cite an entry by its position.
- Put an entry in the track that owns its fix, at the position its impact
  earns. Dependencies do not constrain position: the work order puts a
  prerequisite first wherever it sits.
- Every checkbox carries exactly one **Depends on** bullet, immediately
  before its **Model:** bullet: the IDs it needs, each with a short reason
  if one helps, or `Depends on nothing.` Name an ID in that bullet only as a
  dependency. A track's last entry may say `Depends on every other entry in
  this track.`
- Landing an entry deletes it and edits nothing else in this file: a
  Depends bullet that names a landed ID stays as it is.
- Run `scripts/roadmap.py lint` after editing this file. It checks IDs,
  tracks, the Depends and Model bullets, and dependency cycles.
- Every checkbox carries a **Model:** bullet — a complexity estimate, never a
  sort key — whose value is exactly one of `Opus, Planned`, `Opus, Not
  Planned`, `Fable, Planned`, or `Fable, Not Planned`, and nothing else. Fable
  is for work that changes a contract, spans phases, or has no named lever;
  Opus is for work whose site and rule are both known. Planned means the entry
  needs a plan before code; Not Planned means it is taken as-is. Any rationale
  goes in its own bullet before the Model bullet.

## Working Rule

1. Start with a self-hosted library or small user-facing acceptance case.
2. Record the current failure with a focused test.
3. Implement the smallest compiler change that makes the program honest.
4. Add positive and negative coverage at the owning compiler phase.
5. Run `scripts/check` before marking the task complete.
