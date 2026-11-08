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

Next free ID: **R2050**.

## Ordered Work

### Mojo's Pipeline Order: Parametric MIR And One Elaborator

Track: `pmir`.

Mojo checks a parametric body once, keeps it as parametric IR, checks
lifetimes on that IR, and instantiates everything in one elaborator. Mojito
checks every body once, with its parameters symbolic, and `native::mono`
instantiates every generic body below the MIR waist for both backends; the
VM runs concrete MIR only. The staged plan that closed the gap is
[`docs/parametric-mir-plan.md`](parametric-mir-plan.md); the stage each entry
belongs to is named in its title.

Scope: what remains of the plan's stages, and what the pipeline order still
leaves to close. A defect found on the way is filed by its kind; a
divergence from the pin goes to a catch-up track, however small.

Upstream also shrinks each generic body before it instantiates anything,
and Mojito does not yet. The entries below are that stage, staged in
[`docs/pre-elaboration-optimization-plan.md`](pre-elaboration-optimization-plan.md),
which names the upstream pass each one follows. Each is measured by the
`measure` track's counters and checked by its on/off differential.

- [ ] **R1318 The VM reads a promoted zero-field struct as `<uninit>`:
  its `const uninitmem` ignores the register's type**

  `raise E()` of a fieldless struct `E`, caught and printed, prints
  `<uninit>` on the VM, where native and the pin print the value.
  - Probe: `assets/ok/optional_raising_subscript.mojo` (the VM prints
    `8 <uninit>` for `EmptyOptionalError[Int]()`), and any
    `struct E(Writable)` with no fields raised from `def f() raises E`.
  - Cause: `inline-parametric` inlines the empty `E.__init__`, and
    `mem-2-reg` promotes its `out self` slot, which no store defines, to
    `const uninitmem`, as upstream's `Mem2Reg` reads `undef`.
  - The VM's `const_value` (`crates/mojito-vm/src/backend/vm.rs`) makes
    every `uninitmem` an empty `Value::UninitStorage`, where a struct's
    unwritten value is that struct with no field set yet, as the VM
    already builds for a slot at its first use.
  - Lever: build `uninitmem` from the destination register's type.
  - With the pipeline off (`MOJITO_PRE_ELABORATION=0`) the VM prints the
    value. Found while landing R1310 (2026-10-23).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2017 With the pre-elaboration pipeline off, a call to a
  `Some[Indexer]` parameter reaches the elaborator with the anonymous
  parameter unsolved**

  With `MOJITO_PRE_ELABORATION=0`, `values[Int32(1)]` over a
  `List[Int]` fails with "unsupported monomorphizing
  `List.__getitem__$ov$Some$u5B$Indexer$u5D$$Indexer`: solution for
  `Some[Indexer]` is not concrete", where the pin and the default
  pipeline print `20`.
  - Upstream's `Some[Indexer]` is an infer-only type parameter that each
    call binds to its argument's type; the call reaching the elaborator
    keys the parameter by the opaque `Some[Indexer]` instead.
  - With the pipeline on, `inline-parametric` inlines the accessor and
    hides the call, so only the pipeline-off path, which the
    `pre_elaboration` corpus group runs, meets it.
  - Probe: the program above, and `assets/ok/conversion_traits_library.mojo`
    with `MOJITO_PRE_ELABORATION=0` (its `pick[T: Indexer]` reaches the
    same call), which `tests/focused/list.txt` leaves out until this lands.
  - Found while landing R1999 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2018 With the pre-elaboration pipeline off, comparing a
  `Scalar[DType.bool]` with a `Bool` reaches the VM unconverted**

  With `MOJITO_PRE_ELABORATION=0`, `mask == True` or `mask.__eq__(True)`
  over `var mask = SIMD[DType.bool, 1](True)` fails with "Type error:
  cannot compare Bool and Bool", where the pin and the default pipeline
  print `True`; `mask == Scalar[DType.bool](True)` runs.
  - The VM's `values_equal` (`crates/mojito-vm/src/runtime.rs`) is handed
    a one-lane `Value::Simd` and a `Value::Bool`: the `Bool` argument's
    implicit conversion to `Scalar[DType.bool]`, which upstream performs
    at the call through `SIMD`'s `@implicit` initializer, is not in the
    MIR the elaborator receives.
  - With the pipeline on, the passes fold the comparison away.
  - Probe: the program above, and `assets/ok/bool_declared_operators.mojo`
    with `MOJITO_PRE_ELABORATION=0` (its `b == mask` line), which
    `tests/focused/list.txt` leaves out until this lands.
  - Found while landing R1999 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1279 A generic struct with a `String` field does not compile:
  `inline-parametric` leaves its copy constructor reading the struct's
  parameter**

  `struct S[n: Int](Copyable, Movable)` holding `var s: String`, only
  built and printed in `main`, stops with "after pre-elaboration pass
  `inline-parametric`: MIR function 'S.__copyinit__' removes parameter
  slot 0 of `S`, which it reads", where the pin runs it. It still fails
  after R1283 and R1278 reworked copy and constructor synthesis
  (2026-10-26), and `MOJITO_PRE_ELABORATION=0` runs it.
  - `remove-unused-params` removes `n` from the synthesized
    `S.__copyinit__` (`$v1.s` loaded, `String.__copyinit__` called), which
    reads none of it.
  - Inlining `String.__copyinit__` splices places rooted at the bare
    receiver type `S` with no arguments.
  - The parameter-use walker (`unread_parameters_of`,
    `mir/param_uses.rs`) then counts `n` as read: its `serialize_seq`
    arm takes a bare self spelling with no struct arguments as
    `LayoutReads::All`, every slot.
  - Recipe: `MOJITO_PRE_ELABORATION=0 mojito emit-mir` the program, then
    `mojito opt --pass remove-unused-params --pass inline-parametric` over
    the text reproduces it.
  - Probes: the same over `T: AnyType`, an `@always_inline("nodebug")`
    written copy constructor (which also names `S.copy`), and
    `S[n](copy=c)` in a generic `dup[n]` (which also names `dup`).
  - A `Movable`-only `S` runs.
  - Found while landing R1050 (2026-10-22).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1291 A method call returning no reference runs its callee on the
  host stack, so recursion through a method overflows where upstream's
  interpreter pushes a frame for every call**

  `prepare_direct_call` pushes a frame for a `call.method` only when the
  method returns a reference; every other method call runs through
  `exec_instr`'s synchronous path (`call_frame`, a nested `drive_frames`).
  - Upstream lowers a method call to an ordinary `kgen.call` of the
    method's function, which its interpreter runs as one more frame.
  - A free-function call and a `CallIndirect` are pushed frames, so the
    gap is the method call alone.
  - Probe: `def down(self, n: Int) -> Int:` returning
    `self.down(n - 1) + self.step` for `n > 0`, else `0`;
    `print(Walker(1).down(80))` aborts with a stack overflow in a debug
    build (`down(60)` prints `60`), where the free-function form runs
    100 000 deep.
  - Probe: a `mut self` method `bump` adding 1 to `self.hits` and
    recursing through `self.bump(n - 1)`: `c.bump(200)` overflows where
    `c.bump(60)` prints `60`; its receiver must be written back through
    `ReturnContinuation::writebacks`.
  - Lever: push a frame for every resolved method call, its receiver and
    `mut` arguments written back through the continuation, and its
    `result_adapter` applied when the frame finishes.
  - Found while landing R1272 (2026-10-23).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1731 A callee with a callable compile-time parameter, or a
  parameter holding a function, is no compile-time application**

  `ApplicableShape::applicable` (`checker/comptime_validation.rs`) drops a
  callee whose own compile-time parameters include a callable
  (`callable_bound`), because a function has no compile-time value: a thin
  or capturing callable binder is filled with `CtValue::Deferred`
  (`retained_specialization_param`, `comptime/params.rs`), which only the
  VM reifies.
  - Upstream holds a function in the parameter domain as its symbol
    attribute (`store_to_mem(@functions_in_module::@"foo()")`,
    `Mojo/test/mojo-parser/exprs/functions_in_module.mojo`).
  - Probe: `g[mk[inc](4)]()` over `def mk[f: def(Int) thin -> Int](x:
    Int) -> W` and `def g[w: W]()` is "not an associated comptime
    expression", where the pin prints `5`. The same call in a signature
    fails the same way; an `Int` result in a body already runs through a
    lifted thunk (`g[ap[inc](4)]()`).
  - Lever: a symbol-valued `CtValue` that the elaborator binds and
    `VmBackend::thaw` turns into the VM's function value; then admit
    `callable_bound` binders and function-typed slots.
  - Related: R92 (a struct's callable-value parameter), R1650 (the
    callable binder's environment erased).
  - Found while landing R1644 (2026-10-30).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1727 A `@fieldwise_init` struct that declares another `__init__`
  has no compile-time fieldwise construction**

  `one[Tag(8, "x")]()` over `def one[t: Tag]()` fails with "not an
  associated comptime expression" when `Tag` is `@fieldwise_init` and also
  declares `__init__(out self, *, copy: Self)`, where the pin prints `x`.
  - `applicable_declarations` (`checker/comptime_validation.rs`) lists the
    synthesized fieldwise `__init__` only when
    `synthesizes_fieldwise_constructor` holds, which is false once the
    struct declares any `__init__`. The run-time construction still calls
    the fieldwise one.
  - Inside a display the element falls back to its frozen struct value
    (`Checker::frozen_struct_value`), so `tup[(1, Tag(8, "x"))]()` in
    `assets/ok/value_parameter_materialized_per_use.mojo` runs only through
    that freeze. Delete the fallback in `display_application` here.
  - Probe: `tup[Tuple(1, Tag(8, "x"))]()` over `p: Tuple[Int, Tag]` fails
    the same way; the pin prints `1`, `x`.
  - Found while landing R1529 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1728 A module display constant is a raw element list, not its
  literal constructor's application**

  `show[L]()` over `comptime L = [1, 2, 3]` and `def show[xs: Array[Int,
  3]]()` fails with "expected Array[Int, 3], found KGENParamList[Int]",
  where the pin prints the elements.
  - A module display constant holds a raw `CtValue::List` or
    `CtValue::Tuple` (`Checker::closed_module_constant`,
    `Checker::frozen_display`) or a lifted application typed as a raw
    parameter list (`Checker::module_lifted_application`). At the pin it is
    the `Array[__literal_size__=…](…)` or `Tuple(…)` application, read by
    `__getitem_param__` and `__len__`.
  - Its reads (`XS[1]`, `len(XS)`, `comptime for x in XS`) match the pin
    today through the raw list. A tuple constant's whole-value use bridges
    through `Checker::aggregate_application`.
  - Probe: `LT[L]` over `comptime L = [5, 1]` and `struct LT[l: List[Int]]`
    is rejected naming `KGENParamList[Int]`; the pin's rejection names
    `Array[Int, Int(2)]`.
  - Probe: `f[Span(L)]()` over `comptime L = [1, 2, 3]` and `def f[c:
    Span[Int, _]]()` is "not an associated comptime expression", where the
    pin prints `3`; `Span(L)` selects no `Array` constructor over the raw
    list (found while landing R1725, 2026-10-30).
  - Probe: `f[M]()` over a module `comptime M = [4, 5, 6]` and the same
    `f` is "expected Span[Int], found KGENParamList[Int]", where the pin
    converts the `Array` and prints the elements (found while landing
    R1712, 2026-10-30).
  - The lever: hold the application at the annotation or the natural type,
    and read it through a display sequence, as a local display binding is
    read (`BoundDisplay`). `module_values` is serialized, so this bumps
    `PACKAGE_FORMAT`.
  - Found while landing R1529 (2026-10-30), from R901's probe.
  - Depends on R1530 and R1643.
  - Model: Fable, Not Planned.

- [ ] **R1730 The bundled `Array`'s list-literal constructor lacks
  upstream's `__literal_size__` parameter**

  The pin applies a list display at its natural type as
  `Array[__literal_size__=Int(2)](Int(5), Int(1),
  __list_literal__=NoneType(None))`, with `out self: Array[Self.T,
  __literal_size__]`, where Mojito applies `Array[Int, 2].__init__(*values,
  __list_literal__)` over `Self.length`.
  - The bundled constructor (`stdlib/std/collections/array.mojo`) has no
    compile-time parameter of its own, so a display's application names
    the instance instead (`Array[Int, 2](5, 1, None)` in a mismatch).
  - The lever: port upstream's signature, then bind `__literal_size__` from
    the display's count in `record_array_literal_construction` and in
    `Checker::literal_application`, which today requires a literal
    constructor with no own parameters.
  - Found while landing R1529 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1948 A callable's signature is emitted in passes, so its
  `where` clauses compile twice**

  An application made before its callee's signature registers reads the
  clauses `compile_application_constraints` (`comptime_validation.rs`)
  emits early, and the registered signature compiles them again and
  publishes them over the early ones, where upstream emits one signature
  on first demand.
  - The lever: emit a callable's signature, its clauses included, on
    first demand, so the early emission and the compile in
    `def_signature_in_scope` / `method_signature_owned_by` are one.
  - The obstacle: an application is selected through `&self`
    (`selected_application`), and a signature registers through
    `&mut self`.
  - Probe: `assets/ok/where_applied_alias_clause.mojo` and its
    `type_error` twin keep their output with the early emission gone.
  - Found while landing R1898 (2026-11-05).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1949 A `contains` element naming a binder is re-read as a type
  by its spelling**

  `typelist_proposition_query`'s `PackContains` arm
  (`comptime_validation.rs`) turns a `ConstraintOperand::Param` element
  back into a type through `ty_from_anno(Type::Named(param.name))`, where
  upstream's parameter emitter reads the element's binder once, when it
  emits the condition.
  - The lever: build the element's type from the `ParamRef` it already
    holds (its binder's `TyKind::Param`), not from its spelling.
  - Probe: `assets/ok/pack_length_runtime_position.mojo`
    (`Us.contains[T]()` over a type binder `T`).
  - Found while landing R1899 (2026-11-05).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1339 A `ref self` body types a reference reached through `self`
  as `immutable`, so `automatic-inline` keeps a mutable caller's call**

  Inside a `ref self` method, MIR spells the reference a subscript or
  field of `self` yields at `immutable`, where upstream's carries the
  receiver's mutability parameter (`origin_of(self)`'s `mut`).
  - The body still returns it as the method's `ref` result, which a
    mutable caller writes through, so no program's output changes.
  - `automatic-inline` keeps such a call, since splicing the body would
    raise the reference's permission (`grants_call_permission`), where
    upstream's `AutomaticInline` inlines it.
  - Probe: `def at(ref self, i: Int) -> ref[self.entries[0]] Int`
    returning `self.entries[i]` over `var entries: List[Int]`: emitted MIR
    types its subscript result `immutable`, and `t.at(0) = 5` stays a call
    in concrete MIR.
  - Probe: `d["x"]` on a `var d: Dict[String, Int]` keeps its
    `Dict.__getitem__` call (`assets/ok/template_method_element_field_result.mojo`).
  - Probe: a `ref` parameter passed on has the same cause:
    `through(ref x) -> ref [x] Int` returning `pick(x)` types the result
    `immutable`, so `through(y) += 3` keeps its call in
    `assets/ok/always_inline_nodebug_reference_results.mojo`.
  - Probe: `Optional.unsafe_value(ref self)` returning
    `self._unsafe_unchecked_value()` keeps its call at a mutable caller.
  - `tests/mir_passes/automatic-inline/ref_result_immutable_kept_for_mut_call.mir`
    pins the rule; a body spelling the receiver's parameter inlines
    (`ref_result_param_mutability_inlined.mir`).
  - Lever: type a reference reached through a `ref` receiver at the
    receiver's mutability parameter, which the splice binds at the call.
  - Found while landing R1271 (2026-10-24).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1340 The `automatic-inline` function pipeline has no
  `HoistTrivialInvariants`**

  Upstream's `AutomaticInline` nests `HoistTrivialInvariants` after its
  first `CSEPass`, and Mojito's `AUTOMATIC_INLINE_PIPELINE` skips it.
  - Upstream hoists each pure, region-free operation whose operands are
    all defined outside an `if` or loop to the earliest point its operands
    allow (`lib/Transforms/HoistTrivialInvariants.cpp`).
  - Probe: a field read of a loop-invariant struct value inside a `while`
    loop of an instance stays in the loop body in concrete MIR.
  - Port it as a body pass over regions, reusing `cse`'s effect-free
    instruction rule, and list it in `AUTOMATIC_INLINE_PIPELINE`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1341 The `automatic-inline` function pipeline has no
  `StackReuse`**

  Upstream's `AutomaticInline` nests `StackReuse` before its second
  `CSEPass`, and Mojito's `AUTOMATIC_INLINE_PIPELINE` skips it.
  - Upstream removes a stack allocation whose value, wherever it is read,
    equals another live allocation's (`lib/Transforms/StackReuse.cpp`),
    tracking each allocation's potential value.
  - Probe: a slot stored from another slot's value and only read before
    either is written again, as an inlined body's argument slot is, stays
    a separate slot in concrete MIR.
  - Port it as a body pass over slots, and list it in
    `AUTOMATIC_INLINE_PIPELINE`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1429 An erased (`_REMOVED_ARG`) instance spells a capturing
  callable type argument without its environment, so `automatic-inline`
  splices a slot type its caller's disagrees with**

  A display of a capturing lambda, `[lambda {var c^} -> Int: c.bump()]`,
  stops at the post-elaboration verifier: the caller declares its slot
  `Array[def() capturing[] -> Int, 1]`, and the inlined `Array`
  constructor, an instance with its size argument removed, types it
  `Array[def() -> Int, _REMOVED_ARG]`.
  - Both print the pin's output with `AUTOMATIC_INLINE` removed from
    `post_elaboration.rs`, on both backends; bisected to `dce3b374`
    (R1271).
  - Lever: keep the callable type argument's `capturing` environment in
    the erased instance's types (`mono/substitute.rs`), so the splice
    types the slot as its caller does.
  - Probe: `assets/ok/function_value_array_display.mojo`,
    `function_value_array_element_binding.mojo`, and
    `shadowed_local_callable_call.mojo` stop at the same verifier, an erased
    `Array[def(Int) -> Int, _REMOVED_ARG]` slot declared `Array[def(Int)
    thin -> Int, 2]`, the `thin` marker lost as the environment is
    (failing at 6831b3df and before R662).
  - Probe: `assets/extensions/ok/template_served_iterable_def.mojo` stops
    there too, a `_DictKeyIter` slot typed with `_REMOVED_ARG` arguments
    its caller declares concretely.
  - Found while landing R1427 (2026-10-25).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1276 A region is a `Vec<MirBlock>` the verifier holds to one
  block, where upstream's `hlcf` regions are `SizedRegion<1>`**

  Since R1274 `mir::verify` rejects a body or region of more than one
  block, but the type still admits any number, so every walker indexes a
  block list it knows holds one.
  - Lever: `MirFunction::blocks` and every region field of `MirInstr`
    (`If`, `MirElif`, `Loop`, `ComptimeIf`, `ComptimeFor`, `MirCase`,
    `Try`) become one `MirBlock` (boxed inside an instruction), and
    `instruction_regions{,_mut}` return blocks; the "body/region holds N
    blocks" findings go with the shape they guard.
  - Probe: `MirBlockId` (`mir/ir.rs`) names nothing but block 0.
  - Probe: the VM's `RegionLevel::block` (`backend/vm/regions.rs`) is
    always 0.
  - Probe: Pliron's `BlockKey` bookkeeping (`lower/regions.rs`,
    `lower/release.rs`) keys blocks of one-block lists.
  - Probe: `verify/drops.rs`'s `Walk::block` index names block 0 only.
  - Probe: the `block N` finding prefix and `verify_artifact`'s block
    locator name block 0 only.
  - The MIR text may keep its `[bb0 { … }]` region spelling: MLIR prints a
    region's entry block label only when it has arguments.
  - Large but mechanical: each of the ~200 `instruction_regions` callers
    loses a loop over a list.
  - Depends on R1274.
  - Model: Opus, Not Planned.

- [ ] **R1343 `Conforms` and `ConformsPack` are one proposition upstream**

  Upstream's `#kgen.type_conforms_to_trait` takes a type or a type list,
  and `TypeConformsToTraitAttr::get` splits a list into an `and` of single
  tests, folds an empty one to `True`, and unwraps a one-element one;
  Mojito keeps two `GenericConstraint` variants.
  - The split survives because a `ConstraintOperand::Param` does not say
    whether its binder is a pack, so the reader cannot tell
    `conforms_to(T, X)` from `conforms_to(Ts.values, X)`.
  - Lever: one `Conforms` whose operand carries that kind, normalized as
    upstream's `get` normalizes it, with MIR text reading
    `conforms_pack` as the merged form.
  - Probe: `tests/mir_passes/inline-parametric/conforms_pack_concrete.mir`
    holds a `conforms_pack` over a bound list, which upstream would hold
    as an `and` of two single tests.
  - Found while landing R1096 (2026-10-24).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1468 `MirDeclarations::copy_is_identity` does not hold of a thin
  function value**

  A `def(...) thin` value is a bare code pointer, trivially
  register-passable as upstream's thin function value is, but
  `MirDeclarations::copy_is_identity` (`mojito-mir/src/mir.rs`) answers
  false for every `TyKind::Func`.
  - `canonicalize`'s `forward_copies` therefore keeps a `value.copy` of
    one, and `mem-2-reg` admits a thin function's slot by a test of its
    own (`thin` in `Facts::candidate`, `analysis/mem2reg.rs`).
  - Lever: answer true for a `TyKind::Func` whose environment is
    `CallableEnvironment::Thin`, then drop `mem-2-reg`'s own test; check
    that the `TrivialRegisterPassable` conformance answer beside it and
    Pliron's `copy_is_identity` users (`lower/support.rs`,
    `lower/instr.rs`) still hold for the two-word function value.
  - Probe: `def apply(f: def(Int) thin -> Int, x: Int) -> Int: return
    f(x)` called with a nested `def` keeps a `value.copy` of the
    parameter's `place.load` feeding the `call.indirect` in `emit-mir`'s
    `apply`.
  - Found while landing R1344 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1469 The inliner binds no function value passed to a run-time
  `def(...)` parameter**

  A `nodebug` callee taking a run-time `def(...)` parameter is never
  inlined at a call passing it a function or a closure, where upstream's
  inliner substitutes the value for the parameter.
  - `MirDeclarations::drop_runs_no_code` (`mojito-mir/src/mir.rs`) answers
    false for every `TyKind::Func`, so `Inliner::bindings`
    (`analysis/inline_parametric.rs`) finds no `read` binding for a
    function constant or a closure of reference captures, though neither
    destroys anything.
  - A closure argument's `capture_accesses` are refused too: only a
    compile-time callable binder's closure is one `instantiated_callee`
    accepts (`accessed_through`).
  - So `canonicalize` never sees the call through the parameter, and its
    fold of a call through a closure of reference captures is reached
    only by a compile-time callable binder.
  - Probe: `run_thin(double, 2)` over `@always_inline("nodebug") def
    run_thin(f: def(Int) -> Int, x: Int) -> Int` keeps its call in
    concrete MIR, even with `f` unused.
  - Probe: `apply(double, 5)` over `@always_inline def apply[F: def(Int)
    -> Int](f: F, x: Int) -> Int: return f(x)` keeps its call (`mojito opt
    --pass inline-parametric` over pipeline-off `emit-mir`); once inlined,
    `fold_bound_calls` turns the bound's `__call__` requirement call into
    the function value's `call.indirect`. `apply(Counter(10), 5)` keeps
    its call too, `Inliner::inlined_blocks` declining it, where
    `witness_of` would fold the call to `Counter.__call__`.
  - Probe: `run_it(get, 1)` over `@always_inline("nodebug") def
    run_it[F: def(Int) -> Int](f: F, x: Int) -> Int: return f(x)` and a
    capture-list `def get(y: Int) {k} -> Int` keeps its call; once
    inlined, `canonicalize` folds the call through the closure to
    `get`'s method call on its storage
    (`tests/mir_passes/canonicalize/indirect_closure_storage.mir`).
  - That case's input no longer loads ("closure environment … binds
    neither a leading reference parameter nor a leading `read` one");
    rebuild it and re-bless its `.expected`, which predates R1837's
    storage `literal.materialize` and `struct.create` pair.
  - Found while landing R1345 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1346 A `read` argument read through a pointer register keeps
  its `nodebug` call**

  A pointer's value is the address upstream passes for its pointee, but a
  MIR place cannot root at a pointer register, so `inline-parametric`
  has no address to bind the parameter to.
  - Probe: `len(self.src[])` in `_SetIter.__len__`, `_SetIter.__next__`,
    `_DictEntryIter.__next__`, and `_VariadicListIter.__next__` keeps its
    `len` call (`ref.read` of the pointer `self.src` loads).
  - Binding the parameter through a fresh pointer-typed slot fails
    verification: `place_base_ty` (`mir/verify/places.rs`) unwraps only a
    `ref` through-root, so a pointer root reports "place terminal type
    List[Int] does not match projected type Pointer[List[Int], …]".
  - Lever: a pointer-typed through-root designates its pointee, in
    `place_base_ty` and in the VM's and Pliron's place walkers; then
    `read_through_register` (`analysis/inline_parametric.rs`) admits a
    pointer register too.
  - Regression set: `assets/ok/pointer_field_deref_index.mojo`,
    `pointer_field_view_for_temporary.mojo`,
    `pointer_field_view_ref_yield.mojo`,
    `pointer_origin_cast_reads_pointee.mojo`,
    `pointer_field_wrapper_iterator_protocol.mojo`, and
    `template_served_loan_carrying_instance.mojo`.
  - Found while landing R1099 (2026-10-24).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1347 A callee returning a reference through a local `ref` slot
  keeps its `nodebug` call**

  `returns_bound_place` admits a returned reference made to a place-bound
  parameter, a pointee, or a static origin, but not one made through a
  local `ref` slot an inner reference-returning call fills, which names
  the caller's place only through that call's result.
  - Probe: `Dict.setdefault` returning `ref.make $local.value`, `$local`
    holding an inner `List.__getitem__` result, keeps its call in
    `assets/ok/dict_methods_growth.mojo`.
  - Lever: follow the local slot's one store to the reference it holds,
    and admit the slot's place when that reference is itself admitted.
  - Found while landing R1099 (2026-10-24).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1452 `inline_cost` prices a callee as `mem-2-reg` leaves it, where
  the pin measures it right after splicing its own callees**

  Upstream's `InlineParametric` measures a callee as soon as its own calls
  are inlined, running no pipeline in between (`onComplete` is empty).
  - So a callee its spliced bodies push past the bound in the first round
    inlines only in the second, into a caller the pin then measures with
    that body and the caller's iterator slot uncleaned, and the caller
    keeps its call.
  - Mojito prices every body as its round's `mem-2-reg` would leave it, so
    it inlines such a caller.
  - Probe: `@always_inline def f(x: Int) -> Int` holding `var r = R(x)` and
    `try: s += r.nxt()`, `except: pass`, over an `@always_inline` `nxt(self)
    raises StopIteration` that raises when `self.end == 3` and otherwise
    returns `W[1](self.end).get()`, an inlined constructor and method,
    keeps its call at the pin and inlines here.
  - Marking that `nxt` `nodebug`, which inlines at any size in the first
    round, makes `f` cost 10 at the pin, which confirms the cause.
  - Probe: now that `Int` arithmetic is `SIMD`'s dunders, an
    `@always_inline` `f(x)` holding `for i in range(x): break` costs 33
    and keeps its call, as at the pin.
  - Probe: one holding `var r = range(x)` then `try: s +=
    Int(r.__next__())` costs 31 and inlines, where the pin keeps it.
  - The lever: price the slot traffic, wrapper loop, and breaks of a body
    spliced since the last `mem-2-reg` as the operations the pin counts,
    calibrated against pin probes.
  - Done when the first probe keeps its call.
  - Found while landing R1286 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1255 `canonicalize` does not turn an `if` into a `select`**

  The generator `Canonicalizer` rewrites an `hlcf.if` whose arms only
  yield values into `pop.select` (`EmptyIfToSelect`, `IfToSelect`,
  `IfYieldSelect`, `lib/Transforms/Canonicalizer.cpp`), and Mojito keeps
  the `if`.
  - Shape: an `if` whose two arms yield different constants, such as
    `x = 1 if c else 2`, stays an `if` with a result; `HoistYieldResults`
    folds only equal yields and a `Bool` true/false pair.
  - MIR's `select` (upstream's `pop.select`) runs on both backends; the
    fix adds the three patterns to `canonicalize/hlcf.rs`.
  - Depends on R1254, before which no structured body reaches the pass.
  - Model: Fable, Not Planned.

- [ ] **R1256 `canonicalize` folds a constant in place, where MLIR's
  folder materializes one per value at the region's entry**

  `OperationFolder` hoists each folded constant to the entry of the
  isolated region and reuses one per value, and `fold::replace_folded`
  turns the folded instruction itself into a `const` where it stands.
  - Shape: an elif condition region holding its own `const bool` is
    trivial to `RemoveStaticCondition` only because `trivial_condition`
    accepts `const`s in the region, where upstream's region holds only
    the yield.
  - Shape: equal constants stay one `const` per use, so instance sizes
    count them.
  - Deciding it changes every instance size, so it waits for the
    structured seam.
  - Depends on R1254.
  - Model: Fable, Not Planned.

- [ ] **R924 Mojito inlines `nodebug` callees the pin refuses**

  The pin keeps a call to a `nodebug` callee holding `"abc"`-literal
  storage, `print("big")`, a `String` local or `var String` parameter, or
  a local whose `nodebug` destructor branches, and `inline-parametric`
  inlines them.
  - The instances hold more inlined code than the pin's, and no output
    differs.
  - Probe: `gen[n]` calls `leaf[n](x)`, whose body adds the shape to
    `comptime assert k < 3`, and `main` calls `gen[5]`; the callee was
    inlined when the error trace has no `("k": 5)` instantiation note.
  - Find the pin's legality rule before matching it.
  - Done when Mojito's veto list matches those probes.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R944 The MIR verifier misses types the elaborator substitutes**

  The verifier's scope and concrete modes read an instruction's types
  through `instruction_places` and `instruction_named_types`, which skip
  sites `mir::visit` reaches.
  - Missed today: `MethodCall.reference_result`, a subscript call's
    `parameter_ty` and `reference_result`, and the `comptime` terminators'
    parameters.
  - Give `mir::visit` a read-only twin (`TypeVisitor`, `walk_instruction`)
    and run both verifier modes over it, then delete
    `instruction_named_types`.
  - The concrete mode may report findings it does not report today; triage
    them in this entry.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1035 A package body's cycle check misses the calls its drops
  add**

  `PackageBodies::on_cycle` walks the lowered bodies, before drop
  elaboration, so a cycle closed by a destructor call
  (`FileHandle.close` dropping a `FileHandle` whose `__deinit__` calls
  `close`) is not seen.
  - Finishing such a body reached itself again and deadlocked once
    `remove-unused-params` followed every callee with an argument (R1013).
  - `PackageBodies::finishing_body` now answers such a reentry as a cycle,
    so the member finished first still reads the other's removals, where
    upstream's call graph puts both in one strongly connected component.
  - The walk should follow the lifecycle members each body's drops will
    run, as the program seam's `bottom_up_order` does over drop-elaborated
    bodies.
  - Found while landing R1013 (2026-10-15).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1112 A `def` declared in a `comptime for` cannot call a sibling
  whose argument is a struct over the loop binder**

  `forwards(x)` whose loop declares `leaf(a: Arr[i + 1], y)` and a `mid`
  calling `leaf(a, y)` fails elaboration with "unresolved value parameter
  `$iter`", where the pin runs it.
  - The unrolled copy's own calls are pointed at their instances by
    `instantiate_region_references`, but `mid`'s instance calls `leaf`
    through the ordinary call arm, whose `infer_call` unifies `leaf`'s
    `Arr[$iter + 1]` against the argument from empty bindings and only
    then inherits the region's binders (`nested_bindings`).
  - Inherit the enclosing and region binders before unifying, as the
    generic region path does through `nested_generators`.
  - Probes: the shape above, with or without `leaf` reading `a`; the
    vector analog `leaf(y: SIMD[DType.int64, i + 1])` and the plain nested
    analog over `Arr[n]` already run.
  - Found while landing R1039 (2026-10-17).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1414 A display's elements are not bound as its literal
  constructor's arguments**

  Upstream checks `[a, b]` as the call `C(a, b, __list_literal__=None)`
  through ordinary overload resolution (`emitConstructorCall`), so each
  element binds the collector's convention like any call argument.
  - Mojito calls the literal constructor the check selected
    (`record_display_construction`), but checks each element by hand as
    consumed (`check_consuming`, "collection display element").
  - MIR's `display_elements` (`mir/calls.rs`) lowers each element as a
    plain value, without the facts a call argument carries
    (`BorrowReadArgument`, `ReadTemporaryArgument`, `ForwardedArgument`).
  - So only an owning collector (`var *values`) works. A read collector
    (`*ts`) leaks each element temporary and copies each place element.
  - Lever: bind the elements through the call-argument recording
    (`record_call_places` over the literal constructor's slots and
    conventions), and lower them through `lower_call_arguments`.
  - Probe: give std `Set` upstream's
    `__init__(out self, *ts: Self.T, __set_literal__: NoneType = None)`
    and `var s: Set[Token] = {Token(5), Token(6)}` destroys `Token` twice
    where the pin destroys it four times.
  - Probe: with that constructor, `{x, y}` over locals of a type that is
    not `ImplicitlyCopyable` is rejected as an implicit copy where the pin
    borrows them.
  - Found while landing R1338 (2026-10-25).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1415 A keyword `@fieldwise_init` construction binds by the
  struct's field list, not by its synthesized `__init__`'s parameters**

  Since R1278 the construction names the synthesized `S.__init__`, whose
  declaration carries its parameter names, where upstream binds keywords
  against the selected `__init__`'s signature like any call's.
  - MIR's `fieldwise_keywords_positional` (`mir/calls.rs`) reorders the
    keywords by `OverloadLookup::fieldwise_parameters`, which reads
    `StructDeclaration::fieldwise_init`.
  - `declared_overloads` (`mojito-checked/src/declaration.rs`) reads it
    too, for a package's interface validation.
  - Lever: bind the keywords through the callee declaration, then delete
    `fieldwise_init` from `StructDeclaration` and `MirStructDeclaration`,
    `fieldwise_parameters`, and its `OverloadLookup`/`PackageOverloads`
    method (a MIR text bump: `text/write.rs`, `text/parse/decls.rs`).
  - Verify first whether the elaborator binds a direct `S.__init__`
    call's keywords by name with no fold.
  - Probe: `P(y=3, x=4)` over `@fieldwise_init struct P` with `x` and `y`.
  - Found while landing R1338 (2026-10-25).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1416 Hand-written MIR still constructs by struct name, resolved
  by arity**

  A `call` naming a struct is rejected by the elaborator, but two passes
  still resolve one "for hand-written MIR", and no verifier rule rejects
  it.
  - `Specializer::constructed_default_target` (`native/mono/specializer.rs`)
    and `inline_parametric`'s `DefaultValue::of`
    (`analysis/inline_parametric.rs`) resolve a bare struct target to
    `"{S}.__init__"` by arity.
  - Pass cases still spell `call { func: Pt }` and the like: `sroa/nested`,
    `sroa/whole_read`, `sroa/parameter_binding_kept`,
    `inline-parametric/default_constructed`, and
    `inline-parametric/owner_origin_argument` under `tests/mir_passes/`.
  - Lever: name the `S.__init__` declarations in those cases, delete both
    fallbacks, and add a `mir::verify` rule (`verify/calls.rs`,
    `verify_call_instruction`) rejecting a call that names a declared
    struct, which replaces the elaborator's rejection.
  - Found while landing R1338 (2026-10-25).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1350 `mem-2-reg` keeps a slot whose stored register another
  instruction also reads**

  A slot stays in memory when the register a `var.store` stores is read
  anywhere else, where upstream's `canPromote` has no such test and its
  `Mem2Reg` promotes every store of one SSA value.
  - `Facts::candidate` (`analysis/mem2reg.rs`) requires each stored
    register to be defined once and read only by the store.
  - `canonicalize` uniques equal constants, so two locals initialized
    from the same literal share one register and both keep their slots.
  - Probe: `main` calling `print(outer(4))` and `print(bumped(4))`, over
    `nodebug` nested `def`s `get() {imm y}` and `bump() {mut c}` (each
    local a copy of the argument), keeps `var.store`s of one `const 4`
    into two slots in concrete `main`. Without the rule `main` folds to
    `print(5)` and `print(10)`, as at the pin.
  - Lever: drop the read-count test for a type whose copy is the
    identity, and check what the closure-of-references case still needs.
  - Found while landing R1105 (2026-10-24).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1939 Concrete `eliminate-dead-symbols` keeps every method of a
  live struct, where upstream removes any symbol nothing references**

  `eliminate_dead_concrete_symbols` (`analysis/dead_symbols.rs`) makes a
  live struct's non-constructor methods live from its type alone, so an
  inlined method nothing calls stays in concrete MIR.
  - Upstream's `EliminateDeadSymbols` after elaboration keeps only what a
    live symbol names; every concrete call already names its callee.
  - Lever: in `Resolution::Concrete`, reach a method only through a use
    or the member table's entries the backends run with no call naming
    them, and check which backend lookup still needs the rest.
  - Probe: `outer(4)` over `var y = x` and
    `def get() {imm y} -> Int: return y + 1` inlines `get()` into
    concrete `main`, yet `outer$get$__storage.get` stays in the concrete
    program with no reference; `def bump() {mut c}` keeps
    `counter$bump$__storage.bump` the same way.
  - Found while closing R1349 (2026-11-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1275 A struct's synthesized `copy` is not `@inline(.always)`, so
  `inline-parametric` keeps the call before elaboration**

  `synthesize_copyable_copy` (`mojito-comptime/src/comptime/synth.rs`)
  gives the `copy` it adds to every `Copyable` struct no decorator, so its
  inline level is `automatic`, where upstream's `Copyable.copy` default
  (`std/traits/copyable.mojo`) is `@inline(.always)`.
  - `inline-parametric` inlines only an `always`, `nodebug`, or `builtin`
    callee before elaboration (`is_inlined_before_elaboration`), so a
    `.copy()` of a generic struct stays a call whose receiver type reads
    the generator's parameters.
  - Lever: give the synthesized method upstream's `@inline(.always)`
    decorator, then measure `benchmarks/instantiation.tsv`.
  - Probe: `assets/ok/generic_struct_slot_instances.mojo` with `Outer`'s
    constructor taking `arr: Arr[Self.n]` and storing `arr.copy()` keeps
    three `outer_nested` instances, where the pin (`@no_inline` on
    `outer_nested`, `mojo build --emit llvm`) defines one `_REMOVED_ARG`
    instance.
  - Found while landing R1266 (2026-10-22).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1351 `lower-arg-conventions` stores an owned argument of any size,
  where upstream leaves one over `mem-to-reg-size-limit` in memory**

  Upstream's `lowerPointerType` (`LowerArgConventions.cpp`) lowers an
  `owned_in_mem` argument to a register only when the target's store size
  of its type is at most `maxInlineSize` (256 bytes); a larger one stays
  behind its address, so `SROA` and `Mem2Reg` never see a temporary for it.
  - `store_arguments` (`analysis/mem2reg.rs`) has no size test, so a large
    register-passable owned argument is stored into a local, split by
    `sroa`, and promoted.
  - Lever: give `lower_arg_conventions` the target's store size (the
    layout `crates/mojito-native-core` owns) and skip an owned parameter
    over 256 bytes; `imm-arguments` keeps none, since upstream's parser
    passes every register-passable `read` argument as `imm`.
  - Probe: `@no_inline def f(var b: Big) -> Int: return b.n + 1`, over a
    `TrivialRegisterPassable` `Big` of a `SIMD[DType.float64, 64]` and an
    `Int` (520 bytes), reads `b.n` as a `field.get` of `b`'s whole value
    in concrete `f`, where the pin keeps `b` behind its pointer argument
    and loads the field through it.
  - Found while landing R1268 (2026-10-24).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R934 A symbol reference is an encoding tag, not a MIR type**

  Each MIR and checked field that names a function is a `String` whose
  serde encoding `mojito_types::symbol_ref` tags, where MLIR's
  `SymbolRefAttr` is a typed attribute.
  - A new symbol-naming field added untagged is invisible to
    `eliminate-dead-symbols`, which then removes a live body.
  - No pass can rename references in place, as MLIR's
    `replaceAllSymbolUses` does, without re-serializing.
  - Retype `MakeClosure::function`, `Const::Function`, every `resolved`,
    `MirSubscriptCall::target`, and the checked
    `CheckedIteratorCall::target` and `CheckedConst` targets as one
    `SymbolRef` type that serializes tagged, beside `FuncRef`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R935 A struct type's name is an untyped string**

  `TyKind::Struct`, `CtValue::Struct`, and `ParamKind::Apply` hold their
  symbol as a `String` that only a `serialize_with` tags as a reference.
  - Upstream's struct type and `kgen.param` application hold a typed
    symbol reference.
  - `TyKind::Struct` has some 630 construction and match sites, so it is its
    own session after the MIR fields.
  - Depends on R934.
  - Model: Opus, Not Planned.

- [ ] **R868 A method instance's bare `Self` erases its owner's origin
  binders**

  `substitute_ty` rewrites a bare `Self` in a method instance to
  `Bindings::self_instance`, whose origin tail R866 erased to
  `Origin::Unbound`, where upstream names the owner's own unspecialized
  parameters (`::Span[$0, $1, $2, $3]`).
  - An explicit spelling (`Span[T, origin]`) already keeps `Origin::Param`.
  - The fix needs `MirStructDeclaration` to carry the struct's origin
    slots, which the checker computes in `struct_origin_slots`; that is a
    MIR text-schema and package-format bump.
  - Both backends erase origins, so no output depends on it.
  - Found while landing R866 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R896 The check types the `paramfor_*` stubs at a cost of about 4 ms
  per stub in a debug build**

  The first `comptime for` over each iterator type types three generic stub
  calls, about 12 ms in a debug build, before
  `Checker::comptime_iterator_elements` caches the element type.
  - A loop over an iterator type already seen costs about 2 ms more than
    the built-in `range` rule did.
  - Measured on `assets/ok/comptime_for_template_served.mojo`: the check's
    `bodies` phase went from 12 ms to 57 ms (`--timings`).
  - Lever: find what a generic call bounded by `Iterator & Copyable &
    Deinitable` costs in `comptime_iterator_protocol`
    (`crates/mojito-checker/src/checker/comptime_iteration.rs`).
  - Release numbers were not measured.
  - Found while landing R791 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R897 Each step of a `comptime for` iterator is its own VM call**

  An iterator loop of N elements runs about 3N+2 compile-time
  applications per instance (`has_next`, `next_iter`, and `next_value` per
  step, then the iterable), where the built-in `range` rule ran none.
  - Over the 111 range-loop fixtures, a debug `mojito run` sweep went from
    12.4 s to 18.1 s.
  - On ten small loops (65 steps), elaboration went from 163 ms to 259 ms.
  - Each application's fragment is built and verified once per instance
    (`Specializer::verified_fragment`), so what remains is the call itself.
  - Levers: `VmBackend::call_concrete`'s per-call setup, and the
    `Bindings` clone per step in `Specializer::trip_elements`.
  - Release numbers were not measured.
  - Found while landing R791 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Measure Instantiation And Test The Passes That Shrink It

Track: `measure`.

How much generic instantiation multiplies a program, and proof that a pass
which shrinks it changes no program's behavior. The passes it serves are in
the `pmir` track, staged in
[`docs/pre-elaboration-optimization-plan.md`](pre-elaboration-optimization-plan.md).

- [ ] **R2030 The elaborator's cost grows with a template's
  `inlined_cf_scope` wrapper regions and superlinearly with its length**

  Upstream also erases an inlined body's wrapper only after elaboration,
  in `AutomaticInline`'s SimplifyCF, so the elaborator must handle those
  regions cheaply rather than have them removed early.
  - Probe: removing only the builtin callees' wrappers before elaboration
    cut `prepare.elaborate` from 246 to 161 ms on 400 scalar `a < b` sites
    (template blocks 458 to 231, concrete instructions unchanged).
  - Probe: a bare `print` of an `Int` falls from 129 to 116 ms the same way.
  - That removal is not the fix: it breaks `assets/ok/simd_round_members.mojo`,
    since `native::mono::select::tail_asserts` relies on the wrapper to skip
    an inlined callee's assert in a dead tail.
  - Probe: 800 calls of a `@no_inline` function take 274 ms of `elaborate`
    self time against 115 ms at 100, with nothing inlined.
  - Lever: add sub-spans under `elaborate`, then find the per-region or
    per-instruction work in `native::mono` that scales with nesting depth or
    block length.
  - Generator: `N` lines of `if a < b: n += 1` (or `if f(a, b): n += 1`
    with `f` `@no_inline`) in a `main` over `var a = 3`, `var b = 7`.
  - Found while landing R1575 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2031 `drops.elaborate` is quadratic in a block's length**

  Drop elaboration's time grows with the square of the instructions in one
  block, whether or not anything is inlined.
  - Probe: 800 calls of a `@no_inline` function take 67 ms, against 2 ms
    at 100.
  - Probe: 800 scalar `a < b` sites take 128 ms, against 4 ms at 100.
  - Generator: as R2030's.
  - Lever: find the per-instruction scan in `analysis::drops` that walks
    the whole block, and make it incremental.
  - Found while landing R1575 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Native Backend

Track: `native`.

The ABI-bump collector is last whatever else moves, because it batches every
change that needs a new `MJRT_ABI_VERSION`.

- [ ] **R1450 An `Int`-literal constant two inlined instances share
  fails native verification**

  `assets/ok/simd_symbolic_surface.mojo` stops in the native backend
  ("builtin.integer or its vector element type does not implement
  interface", at its `inferred_lane(Float32(1.5), Float32(2.0))` call),
  where the VM prints its golden and the parity manifest says
  `exe-differential`.
  - It fails at HEAD before R662 too.
  - Concrete `main` has one `const int_literal(1)` register that the
    inlined `Int32` and `Float32` bodies of `a * b + 1` both add, so the
    `Float32` add reads an integer constant.
  - It takes both instances of `inferred_lane[dt]` and of a second
    generic (`rebound[dt]`, whose `comptime lane = dt` body adds
    `Scalar[lane](1)`) to show; either call pair alone runs.
  - Lever: find the pass that shares the literal across the two bodies
    (`cse` after `automatic-inline` is the first suspect), or type the
    literal at each use as upstream's literal conversion does.
  - Probe: `print(v[2])` over an `int32` vector, then `f[3] *= 2` over a
    `float32` one, fails the same way with no generic at all: `cse` merges
    the index `2` and the multiplier `2` into one `int_literal(2)`, and
    Pliron's `reg_value` caches a pending literal at its first use's type
    (`cache_lazy`), so the `float32` multiply reads an `Int`.
  - Found while landing R662 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1706 An `OptionalReg` over a parametric function type has no
  native layout**

  `var o: OptionalReg[_B] = None` over `comptime _B = def[dtype: DType,
  width: SIMDLength, //](SIMD[dtype, width], SIMD[dtype, width]) thin ->
  SIMD[dtype, width]` prints `False` on the VM and at the pin, and is
  rejected natively with "type has no native layout yet".
  - `docs/native-abi.md` gives `TyKind::GenericFunc` no representation, so
    `Variant[NoneType, G]` has no layout.
  - Upstream lays out a `!kgen.generator` value. A layout for one here is
    the lever.
  - A `comptime if` over such a value folds on the VM's compile-time
    evaluation and already runs natively (`assets/ok/std_plugin_hooks.mojo`).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1477 Natively, a generic vector-plus-literal fails verification
  once two dtypes instantiate it**

  `def h[dt: DType](x: Int) -> SIMD[dt, 1]: return SIMD[dt, 1](x) + 1`,
  called as `h[DType.float32](3)` and `h[DType.int64](3)`, stops with
  "IR verification failed: Op has different operand types", where the VM
  and the pin print `4.0` and `4`.
  - Each instance alone runs, and `SIMD[dt, 1](x) + SIMD[dt, 1](1)` runs
    at both dtypes, so the instances appear to share the lowered type of
    the literal operand.
  - Probe: the float instance beside the stdlib's own integer one, `var y
    = h[DType.float32](3)` then `print(String("x"))`, stops with
    "Forwarded operand at 0 is of type builtin.fp32, but should've been
    builtin.integer i64".
  - Found while landing R1380 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1265 pliron 0.20.0's DCE panics when it queues one block
  argument twice**

  pliron's `DCEPass` queues each operand of an erased op or dead block
  argument once per occurrence, and erasing a block argument a second time
  panics in `Value::find_index`.
  - A dead phi whose edges carry one block argument twice, which
    `mem-2-reg`'s region values now leave in `match` lowering
    (`assets/ok/match_sequence_fixed_length.mojo`), was the first shape.
  - Mojito's `EraseDeadBlockArgs` (`crates/mojito-pliron/src/pipeline.rs`)
    erases every dead argument before DCE runs, so none starts the chain.
  - A dead op with one block argument as two operands, or a dead argument
    an erased op exposes whose edges repeat a block argument, still
    panics.
  - The fix runs a DCE that skips an already-erased argument, as pliron's
    does for an erased op, until a pliron release dedups its queue; draft
    the upstream report as a local file.
  - Found while landing R927 (2026-10-21).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1069 Natively, a place's type carries an origin that varies from
  run to run**

  Problem: compiling `assets/ok/pointer_tuple_field_write.mojo` twice gives
  a `Pointer` field place in `main` the origin `Self.o1` on one run and the
  local's place origin on the next, so the lowering sees two types for one
  place.
  - `lower_make_ref` now compares a borrowed pointer's type modulo origin,
    so the emitted IR no longer differs, but any other type equality over
    a place's type is exposed.
  - The nondeterminism is likely a hash-ordered lookup in `place_address`'s
    type resolution.
  - Done when the `.ll` of that fixture is byte-identical across runs with
    the comparison made exact.
  - The fixture's instantiation counts vary too (498 or 504 concrete
    instructions on one binary), so `scripts/instantiation-report
    --check` can fail on it with no change (2026-11-01).
  - Found while landing R986 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1074 Natively, an inlined small `@always_inline` callee's code
  is located at the call, not at its own lines under an `inlinedAt` scope**

  `inline-parametric` gives every register a plain `@always_inline` callee
  brings the call's location, as it gives a `nodebug` callee's, where the
  pin keeps the callee's own locations under an `inlinedAt` scope naming
  the call.
  - A location has no scope chain: `SourceSpan` is one file and span, and
    the pliron debug attach degrades a function whose call locations lie
    in another file, which an inlined library callee's would.
  - Give a MIR location an optional inlined-at location, emit it as a
    `DILocation` with `inlinedAt` over the callee's subprogram, and keep
    the call-count correlation counting the caller's own calls.
  - Probe: `automatic-inline` locates every plain `def` it inlines after
    elaboration at the call too, as `add` called from `main` in
    `tests/mir_passes/automatic-inline/small_recursive_and_no_inline.mir`,
    where upstream's `AutomaticInline` keeps the callee's locations.
  - Done when `clamp` in `assets/ok/always_inline_small_calls.mojo` steps
    at its own lines natively and the heavy lane's debug premise holds.
  - Found while landing R923 (2026-10-16).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R987 Native code drops every function's inline level**

  A `nodebug`, `builtin`, or `always` callee the elaborator leaves as a
  call, such as `List.__len__` reached through `len`, stays an ordinary
  LLVM call, and a `never` callee is not marked `noinline`.
  - The concrete program still holds 868 `List.__len__` and 523
    `Span.__len__` instances, reached through trait-bound calls that only
    the elaborator resolves.
  - Confirm the attributes the pin's LLVM IR carries for each level, then
    emit them from the declaration's `inline` in Pliron's function
    lowering.
  - Done when a `--backend pliron` `.ll` of a `len(xs)` probe marks
    `List.__len__` as the pin does and the parity lane is unchanged.
  - Found while landing R939 (2026-10-15).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2039 Natively, scalar `Float32` and `Float16` arithmetic computes
  at f64 and rounds, where upstream emits it at the lane width**

  `lower_narrow_float_binop` (`lower/binops.rs`) widens a scalar `Float32`
  or `Float16` `+`, `-`, `*`, or `/` to f64, operates, and truncates, while
  the pin writes one `fadd float`; a `pop` op's fast-math flags land on the
  f64 instruction.
  - The result is the same: f64 holds more than twice either lane's
    precision plus two bits, so a basic operation rounded twice equals one
    rounded once. This is shape, not semantics.
  - `simd_float_binop_vectors` (`lower/simd.rs`) already computes `Float32`
    vectors at the lane width; `Float16` vectors widen too
    (`lower_simd_binop`, `pop_float_value`).
  - Probe: `pop.add` with `#pop.fmf<nnan>` over two `Float32` lowers to
    `fpext`, `fadd nnan double`, `fptrunc`, where the pin writes
    `fadd nnan float`.
  - Probe: `Float16(1.5) + Float16(2.0)` lowers through `double`, where the
    pin writes `fadd half`.
  - Found while landing R1600 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R936 A program cannot declare a native root with `@export`**

  Upstream's `@export` marks a function as a root of its
  `EliminateDeadSymbols`, but Mojito rejects the decorator.
  - A native entry compiled by name (the scalar sweep's `compute`) stays
    live only because its caller passes `Compiler::with_exports`.
  - Confirm at the pin which `@export` spellings it accepts, then accept
    those, record the flag on the function's declaration, and root the
    pass at it as well as at the compilation's exports.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1974 Natively, a run-time read of a memory-only struct
  parameter value fails as an untyped variable**

  `if x: print(x.value())` in `def f[x: Optional[String] = None]()` runs
  on the VM, but the Pliron backend rejects the instance ("unsupported
  untyped variable `x`"), where upstream materializes the parameter
  value into a typed temporary.
  - The concrete MIR's variable holding the materialized parameter
    carries no `var_tys` entry, which the VM does not need and
    `var_lower_ty` (`mojito-pliron/src/lower/emit.rs`) does.
  - A `[x: String]` parameter runs natively; `Optional[Int]` and
    `Optional[String]` ones fail.
  - Probe: `f[3]()` over `def f[x: Optional[Int] = None]()` and
    `f["b"]()` over the `Optional[String]` form, natively.
  - Probe: `var v = x.or_else("none")` and `var seen = True if x else
    False` over the `Optional[String]` form fail natively the same way;
    they pass the verifier since R1934 (2026-11-08), and join
    `assets/ok/parameter_value_consuming_receiver.mojo` once this lands.
  - Then mark `assets/ok/nonmaterializable_converts_through_target.mojo`
    `exe-differential` in the parity manifest.
  - Found while landing R1864 (2026-11-06).
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
  - Trap category 2 (`rt_abi::TRAP_POW_EXPONENT`) has no emitter since
    `**` became `SIMD.__pow__`'s body: retire its runtime message row,
    its `docs/native-abi.md` entry, and its `tests/native_abi_test.rs`
    pin.
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
  - R1799 already moved a one-field `TrivialRegisterPassable` struct's
    signature classification to its field (`docs/native-abi.md`
    §Calling convention) without a version bump, since only
    generated code meets it. The bump records that rule change too.
  - A multi-lane `SIMD` lays out aligned like one lane
    (`docs/native-abi.md` §Layout, `LayoutCx::layout_of`), where the pin
    lays it out as LLVM's vector type: `align_of[SIMD[DType.float32,
    4]]()` is `4` against the pin's `16`, and `size_of` of a struct `{UInt8,
    SIMD[DType.float32, 4]}` is `20` against `32`.
  - Probe: `size_of[SIMD[DType.float32, 3]]()` is `12`, where the pin
    prints `16`: LLVM pads a `<3 x float>` to its 16-byte alignment, the
    next power of two of its 12-byte store size.
  - The fix aligns a multi-lane `SIMD` as its LLVM vector, which moves
    every struct layout holding one.
  - A later task that needs an ABI bump joins this entry rather than
    getting its own.
  - The signature classification changes the native calling convention itself,
    and the ABI version, `docs/native-abi.md`, the runtime, and the parity
    harness all move together.
  - Depends on nothing. Every later change that needs an ABI bump joins this
    entry instead of waiting on it.
  - Model: Fable, Planned.

- [ ] **R2032 Natively, a raising initializer read as a function value
  is unsupported**

  Problem: `var e: def(Int) thin raises -> E = E.__init__` then
  `print(e(2).a)`, over `__init__(out self, k: Int) raises`, prints `2` on
  the VM and at the pin, while the native backend reports "a function value
  of the raising initializer `E.__init__`".
  - An initializer's `out self` is the value's result, so the `invoke`
    thunk (`ensure_thunk`, `emit_thunk_body` in
    `mojito-pliron/src/lower/support.rs`) passes its out-pointer as the
    receiver (`ThunkReceiver`).
  - A raising initializer returns a tagged outcome whose ok payload is
    `None`, while the function type's outcome carries the struct inline.
  - Lever: give the thunk its own target outcome, point the receiver at
    the contract outcome's ok slot, and copy the tag and error across.
  - The rejection is in `check_contract_target`
    (`mojito-pliron/src/lower/calls.rs`).
  - Found while landing R1796 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R435 Natively, `Error` over a local `comptime` string is
  unsupported**

  Problem: `comptime msg = "boom"` then `raise Error(msg)` runs on the VM
  and at the pin, while the native backend reports "unsupported string value
  in register".
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1647 Natively, a frozen pointer is a fresh heap allocation at
  every read**

  A `StaticString` parameter read in a loop allocates its bytes once per
  iteration natively and never frees them, where the pin addresses one
  constant global.
  - Pliron's `store_parameter_value` (`mojito-pliron/src/lower/consts.rs`)
    materializes every frozen `CtValue::Pointer` with `emit_alloc`
    (`mjrt_alloc`) and writes its slots, at every materialization.
  - The pin holds a static-origin view's memory as a `const_global`
    (`#interp.memory_handle<…, const_global>` in its diagnostics), with a
    trailing NUL byte that Mojito's frozen bytes lack.
  - Lever: emit an immutable frozen allocation as one private constant
    global per distinct allocation, and address it.
  - Probe: `for _ in range(3): total += s.byte_length()` in `def f[s:
    StaticString]() -> Int`, whose `--emit ll` calls `mjrt_alloc` inside
    the loop body, with no free.
  - Found while landing R1641 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Catch Up To Current Mojo: Ownership, Origins, And Destruction

Track: `ownership`.

Every catch-up track closes a gap between Mojito and the pinned Mojo. Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

- [ ] **R1976 A subscript getter or `__contains__` keeps its
  register-passable `read self` receiver's place, so the owner outlives the
  call, where upstream passes it by value**

  `a.p[2]` and `1 in a.p`, with `p` a `TrivialRegisterPassable` field of a
  `W` that prints in its destructor, print the dunder before `del` here,
  where the pin prints `del` first.
  - These receivers are lowered outside `lower_receiver_value`
    (`mir/calls.rs`), which since R1570 drops the place of a receiver the
    checker judges `RegisterPassableRead`.
  - The subscript lowerings keep `base_place`/`object_place`, and the
    `in` lowering (`contains_call`, `mir/lower_expr/expr.rs`) keeps its
    receiver's place.
  - Check that the checker's `read_place_receiver` fact reaches these
    receivers too, then apply the same rule.
  - Probe: `var r = a.p[2]` prints `del 3`, `getitem`, `3` at the pin.
  - Probe: `var r = 1 in a.p` prints `del 2`, `contains`, `True` at the pin.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1682 A local read through an untracked pointer after its last use
  reads `None` on the VM**

  `var k = Pointer(to=x)._get_kgen_pointer()` then
  `print(Pointer[_, ImmUntrackedOrigin](_mlir_value=k)[])`, with no later
  use of `x`, prints `None` on the VM, where the pin and `--backend pliron`
  print `5`.
  - An untracked pointer and a `!kgen.pointer` lend nothing, so `x`'s
    `drop.var` lands before the read, and the VM's `drop.var` clears the
    frame slot the pointer's handle designates.
  - The pin destroys a trivial `Int` at its last use too, but its storage
    stays in the frame, as it does natively.
  - Probe: `Pointer(to=x).unsafe_origin_cast[MutUntrackedOrigin]()` then
    `print(p[])` prints `None` the same way.
  - Probe: the element pointer `pop.array.gep` takes into a struct
    field's array, read through `Pointer[_, ImmUntrackedOrigin](
    _mlir_value=…)[]` with no later use of the struct, stops with "use of
    a value after it was moved", the struct slot's tombstone.
  - Found while landing R1316 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1977 A field read out of a struct temporary does not tie the
  temporary's life to how the call takes the field**

  At the pin a field of an expression temporary is a projection of it: a
  register-passable `read` parameter loads the field at call setup, so the
  temporary dies before the call, and a by-address parameter lends the
  field, so it dies after. Mojito's `field.get` of the owned register gets
  neither rule.
  - Probe: `show(R(5).x)` over `def show(i: Int)` and a `RegisterPassable`
    `R` printing in its destructor prints `show 5`, `del 5`; the pin
    prints `del 5` first.
  - Probe: `print(R(5).x)` prints `del 5` before `5`; the pin prints it
    after, as `print`'s variadic pack lends the field.
  - Probe: `print((R(3) if c else R(4)).x)` misorders the same way.
  - `gen(R(6).x)` over a generic `read` parameter already matches.
  - R1570's call-setup load reads `single_hop` provenance, which only a
    scalar read of a variable's place records (`RegisterLoanStep::new` in
    `register_loans.rs`); a `field.get` of an owned register records none,
    and a `lit.ref.pack` of its result does not retain the register.
  - Found while landing R1603 (2026-11-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R473 A discard `_ = e` binds its value into a slot, so a place is
  copied or moved into it**

  Problem: over a `Copyable`-only `Q` printing in its destructor, `var x =
  Q(1, "y")` then `_ = x` is rejected ("cannot be implicitly copied …
  (discard assignment)"), where the pin runs it and prints `del 1`.
  - The checker's discard arm (`StmtKind::Assign` with `_` in
    `checker/statements.rs`) calls `check_consuming`, so the value must be
    copyable or transferred.
  - HIR lowers the discard as a `Bind` into a fresh `$discard_s` slot
    (`mojito-hir/src/hir.rs`), so a place's value is copied, or moved at its
    last use, into storage that dies at once.
  - The pin's discard takes nothing from a place, which dies at its last
    use as usual.
  - Probe: over `xs: List[D]` (`D` only `Copyable`, printing its copy and
    destructor), `_ = xs.__getitem__(0)` prints `copy 1`, `del 1`, where the
    pin prints nothing, and `_ = xs[0]` is rejected as above.
  - Probe: over an `ImplicitlyCopyable` `C` printing its move initializer
    and destructor, `_ = w` at `w`'s last use prints `move 3`, `del 3`,
    where the pin prints `del 3` alone.
  - An `ImplicitlyCopyable` `_ = x` at `x`'s last use prints one `del`, as
    at the pin.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1446 A move into a variable that dies unread still runs the move
  initializer**

  Problem: `var u2 = t2`, with `u2` never read and `t2` dying there, prints
  `move 2`, `del 2` over an `ImplicitlyCopyable` `C` printing its move
  initializer and destructor. The pin warns that `u2` is never used and
  prints `del 2` alone.
  - Copy elision already turns the copy into `t2`'s move.
  - No rule deletes a move into a slot that is destroyed before any read.
    `delete_dead_copies` (`analysis/copy_elision.rs`) deletes only a copy
    whose one use is the `drop.reg` right after it.
  - The fix is that rule's slot form: a `var.store` of a moved value whose
    slot's next use is its drop becomes the source's own drop.
  - Found while triaging R473 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R731 A transferred call temporary skips the move initializer the
  pin runs**

  Problem: `var a = mk()^`, over an `M` whose move initializer adds 1000,
  prints `a 5`, where the pin prints `a 1005`.
  - Found while landing R723 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R738 A field transfer into a binding skips the move initializer
  the pin runs**

  Problem: `var t = self.b^` in a `deinit self` method, over an `M` field
  whose move initializer adds 1000, prints `t 2`, where the pin prints
  `t 1002`.
  - The field's `MovePlace` relocates the bytes on both backends.
  - A field transfer an owned parameter takes (`take(self.a^)`) already
    matches the pin, which runs no move initializer there.
  - Found while landing R730 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1047 A field move names no move constructor; the backends
  relocate its bytes**

  `place.move` of a struct-typed field (`self.c = existing.c^`) carries no
  `constructor`, so the VM transfers the value and Pliron copies the bytes,
  where upstream's move insertion calls the field's `__moveinit__`.
  - Probe: `var u = t^`, over a `T` holding a `C` whose move initializer
    adds 100, prints `move / 101`, where the pin prints `move / move / 201`:
    `T`'s synthesized `__moveinit__` (R1044) runs, and its `place.move` of
    `c` runs no `C.__moveinit__`.
  - A hand-written `__moveinit__` spelling `self.c = existing.c^` skips it
    the same way.
  - Probe: `var h = g^` over `def g() {var t^} -> Int` and a `Tracked`
    whose move initializer prints prints nothing, where the pin prints
    `move 3`: the closure storage's synthesized `__moveinit__` moves its
    field the same way.
  - Name a `constructor` on `place.move` as R1017 did for `var.use move`,
    respell it in `Specializer::name_lifecycle_calls`, and run it in both
    backends.
  - Found while landing R1017 (2026-10-16); the cause was named while
    landing R1044.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2027 `Pointer.unsafe_take_pointee()` and `unsafe_write` of a
  call result run no move initializer, where the pin runs one each**

  `pointer.take` and the `ref.write`/`store` an `unsafe_write` lowers to
  carry no `constructor`, so the value is relocated, where upstream's
  `__get_address_as_owned_value` and its `init_pointee_move` each move it.
  - Probe: over an `M` whose move initializer adds 1 and prints,
    `pb.unsafe_write(pa.unsafe_take_pointee())` prints nothing where the
    pin prints `move 12 / move 13`, and `var x = pb.unsafe_take_pointee()`
    prints nothing where the pin prints `move 14`.
  - `unsafe_write(x^)` and `unsafe_write_move_from` already run one, as
    at the pin.
  - So `Tuple.reverse`, `concat`, and `consume_elements` run no element
    move initializer.
  - Same family as R1047: name the constructor on the instruction and
    run it in both backends.
  - Found while landing R341 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2029 Taking or destroying a pointee with a destructor through an
  origin-bearing pointer is rejected outside five bundled modules, where
  the pin accepts it**

  `check_pointer_take_source` (`checker/method_calls/builtin_types.rs`)
  requires a trivially destructible element for `unsafe_take_pointee`,
  `unsafe_deinit_pointee`, and `unsafe_write_move_from` unless the origin is
  untracked or the call is in a module `is_bundled_private_storage_source`
  lists (`tuple`, `_enum`, `variadics`, `maybe_uninit`, `variant`).
  - Probe: `var b = Pointer(to=a).unsafe_take_pointee()` over a local `N`
    with a destructor, then `Pointer(to=a).unsafe_write(N(2))`: the pin
    prints `took 1 / del 1 / del 2 / end`, Mojito reports "requires a
    trivially destructible element".
  - Lifting the gate means the ownership analysis must accept a
    pointer-side take of a tracked slot as upstream does, leaving soundness
    to the caller.
  - Found while landing R359 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1048 Returning a named `out` result runs its move initializer,
  which the pin does not**

  Problem: `def mk2(out c: C): c = C(7)` prints `move` at `var b = mk2()`,
  where the pin prints nothing.
  - `materialize_named_result_return` (`mir/nested.rs`) returns the slot
    with `var.use move`; upstream initializes the caller's storage in
    place, so it should be a `forward`.
  - Check `inline-parametric`'s `Returned::Out` result move the same way.
  - `return c^` from a local moves once at both.
  - Found while landing R1017 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R739 A Tuple display over a dying copy runs one move initializer
  where the pin runs two**

  Problem: `(M(copy=b), 3)` at `b`'s last use prints `t 1002`, where the
  pin prints `t 2002`.
  - `(b^, 3)` matches the pin at `t 1002`.
  - Tuple's initializer stores its pack whole without element move
    initializers (R2025), and a display element is not a forwarded
    argument.
  - Found while landing R730 (2026-10-13).
  - Depends on R2025.
  - Model: Opus, Not Planned.

- [ ] **R732 A `@fieldwise_init` construction stores its `var` arguments
  without the fields' move initializers**

  Problem: `P(M(3), M(4))` over `@fieldwise_init struct P` with two `M`
  fields leaves them `3` and `4`, where the pin runs `M`'s move
  initializer into each and leaves `1003` and `1004`.
  - Found while landing R723 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R828 A `for` loop's iterator outlives its last `__next__` on a
  `return` path**

  Problem: `for x in Tracked(1): try: if x == 2: return x finally: ...`
  prints `returning 2`, `iter dropped at 3`, where the pin destroys the
  iterator first.
  - HIR lists the loop's binding, iterator, and owned source on the return
    (`Cfg::ret`), and MIR keeps them alive to it with `lifetime.keep_alive`
    (`lower_stmt.rs`).
  - Removing the keep-alives breaks every `Dict` fixture: a reference the
    loop yields holds no loan on the loop's source, so `Dict._find_index`
    reads a dead `List`.
  - Fix: the loop binding's reference establishes a loan on the iterator's
    roots, then the keep-alives and HIR's loop-owned list go.
  - Since R884 the `finally` is copied before the return's keep-alives, so
    its copy can read what the loop yields: the iterator now dies after
    the `finally` (`returning 2`, `finally 2`, `iter dropped at 3`), where
    the pin prints `iter dropped at 3` before `returning 2`.
  - Found while landing R826 (2026-10-14).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R718 A struct prefix operator's operand lives as long as its
  result**

  Problem: `print((-S(16)).get())` prints `del -16`, `del 16`, where the
  pin prints `del 16`, `del -16`.
  - A struct `UnOp` is missing from `call_result` in
    `RegisterLoanStep::new` (`register_loans.rs`), so its result register
    inherits the operand slot's provenance.
  - A discarded `-S(16)` statement matches the pin only by this accident:
    once the operand dies at the dunder's return, the statement's result
    must be bound in place (`produced_in_place`) to keep dying first.
  - Found while landing R714 (2026-10-13).
  - Depends on R719.
  - Model: Opus, Not Planned.

- [ ] **R715 A transferred temporary at a read parameter is never
  destroyed**

  Problem: `f(S(20)^)` over `def f(s: S)` prints `f 20` and no `del`,
  where the pin prints `f 20, del 20`.
  - A `^` argument is excluded from `read_temporary_arguments`, so its
    owned operand gets no call-result slot (`Flatten::operand_c`).
  - Found while landing R713 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R689 A direct call's keyword arguments are set up in source order,
  not parameter order**

  Problem: `own2f(y=d2, x=d3)` over `def own2f(var x: D, var y: D)`, with a
  printing copy initializer, prints `copy 3` then `copy 2` at the pin and
  `copy 2` then `copy 3` in Mojito.
  - A method call orders its setup reads by its `CheckedCallContract`
    (`close_call_setup`); a direct call carries no contract, so its reads
    keep source order.
  - The Mojo-shaped fix records the `mojito_ast::call` binding's slot order
    on the checked direct call, as a checked fact rather than a MIR
    re-binding.
  - Found while landing R681 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R680 A mutable reference result through a function value is not
  an assignment target**

  Problem: `var f = field; f(w) = 7` over `def field(mut w: W) ->
  ref[origin_of(w.t)] Int` prints `7` at the pin, but Mojito rejects it:
  "expression must be mutable in assignment ('immutable reference returned
  by call 'f()'')".
  - A nested `def inner(mut w: W) -> ref[origin_of(w.t)] Int` with
    `inner(w) = 7` is rejected the same way, where the pin accepts it.
  - The rejection comes from the call-as-place arm in
    `crates/mojito-checker/src/checker/indexing.rs` (the "immutable
    reference returned by {callee}" site): an indirect call's reference
    result resolves as immutable even when its contract's `ref_return`
    names a `mut` parameter.
  - When it lands, add both cases to an `assets/ok` fixture and run it
    natively too.
  - Probe: a module-level `def fa(mut p: P) -> ref[p.a] Int` called
    directly, `fa(p) = 7`, and as a destructuring leaf, `(fa(p), m) = (5,
    6)`, are rejected the same way; the pin prints `7`, and `5 6` for the
    pair.
  - Found while landing R676 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1878 A `ref` binding of a call's reference result holds no loan
  against a move of its origin**

  `ref a = fr(p)` over `def fr(ref s: P) -> ref[s] P`, then `var b = p^`
  and a write through `a`, fails MIR verification after `sroa` ("drop.var
  of slot $v0 ('p') that some path reaching it does not hold"), where the
  pin reports "use of uninitialized value 'p'" at the write.
  - `ref a = p.x` in the same program gets the pin's diagnostic, so the
    gap is the hidden handle slot a call's reference result binds through
    (`Lower::materialize_reference_result_place`, `mir/calls.rs`): the
    moves analysis does not see `a`'s later use as a use of `p`.
  - The program is rejected either way; the phase and the message are
    wrong.
  - Probe: the same with `ref a = reflect[P].field_ref[0](p)` and `a = 5`
    fails the same way.
  - Found while landing R77 (2026-11-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R691 An accessor chain over a temporary is read where it is
  written, not at call setup**

  Problem: `two(ident(Span(xs))[0], bx(xs))`, with `bx(mut l)` adding 100
  to `l[0]`, prints `two 101 0` at the pin and is rejected by Mojito with
  "access to 'xs' conflicts with live reference '$arg_loan_r11'".
  - R685 reads a reference-result operand at its call's setup only when
    its accessor chain roots at a named place (`setup_reference` in
    `crates/mojito-mir/src/mir/calls.rs`): the VM resolves a view's
    referent through the receiver, and a temporary receiver's slot dies at
    its last use.
  - The temporary `Span(xs)` argument's anchor slot (`$arg_loan_r`) holds
    an exclusive loan on `xs` to the statement's end, so the later `mut`
    argument conflicts with it even before the read's placement matters.
  - Found while landing R685 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1505 A destructuring leaf's reference lvalue borrows its root
  exclusively until its store**

  `(r[0], r[1]) = (5, 6)` over a struct whose only accessor is a `ref`
  getter is rejected ("access to 'r' conflicts with live reference
  '$call_ref_r4'"), where the pin forms both lvalues, then stores through
  each, and prints `5 6`.
  - `lower_destructure` (`mir/lower_stmt.rs`) forms every place leaf before
    the value, as upstream's `TupleDLValue` does, and a getter leaf's handle
    is materialized by `materialize_call_reference_place`, whose loans are
    exclusive.
  - The second leaf's getter call takes `r` mutably while the first
    handle's loan is live; upstream's lvalue names an origin and excludes
    nothing until the store, as a `ref` binding's setup loans do here
    (`reference_binding_loans`).
  - Probe: the store above; a single getter leaf, `(r[0], m) = (5, 6)`,
    already runs (`assets/ok/destructure_subscript_targets.mojo`).
  - Found while landing R1374 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R673 A read register-passable value's origin is accepted outside a
  result**

  Problem: `def g(items: Int, ref[origin_of(items)] other: Int) -> Int`
  checks and runs, but the pin rejects it: "value of type 'Int' doesn't
  have a memory origin in origin specifier".
  - The pin rejects `origin_of(p)` over a read trivially register-passable
    `p` in any origin position, a body-local annotation included; a result
    is already judged by `Checker::check_result_origins`
    (`crates/mojito-checker/src/checker/origins/sig.rs`).
  - The lever: judge every origin specifier's `origin_of` place the same
    way, at `validate_origin_expr` and the body's origin resolution, with
    `Checker::register_passability`.
  - Found while landing R668 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1066 A result `ref [x]` over a read register-passable parameter
  is accepted**

  `def get(x: Int) -> ref [x] Int: return x` checks and runs, but the pin
  rejects it: "value of type 'Int' doesn't have a memory origin in origin
  specifier".
  - `Checker::check_result_origins` already rejects the
    `ref [origin_of(x)]` spelling of the same result
    (`assets/type_error/result_origin_read_register_passable.mojo`); the
    bare-place shorthand `[x]` escapes it.
  - The lever: judge a shorthand origin place as `origin_of` of it, with
    `Checker::register_passability`.
  - Done when the shorthand is rejected with the pin's message and a
    `type_error` fixture pins it.
  - Found while landing R953 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1986 A store through a `ref` result whose origin mutability is a
  function's own parameter is accepted**

  `def poke[m: Bool, //, o: Origin[mut=m]](p: Pointer[Box, o]): p[][0] =
  9`, over a `Box` whose `__getitem__(ref self, i: Int) -> ref[self.v]
  Int`, runs and prints `9`, where the pin rejects it: "expression must be
  mutable in assignment".
  - Cause: `require_writable_reference`
    (`checker/method_calls/receiver_effects.rs`) treats every
    `Mutability::Param` as writable. It records the write for a judgment at
    each instantiation only when the parameter is the enclosing struct's,
    so a function's own origin parameter is never judged.
  - Upstream judges the binder inside the generic body and rejects there,
    as `check_pointer_write` already does for a direct `Pointer` store
    ("write through a Pointer whose origin mutability is not known here").
  - Probe: the same body over `List[Int]` once R1985 drops
    `List.__setitem__` (`assets/type_error/pointer_param_symbolic_origin_write.mojo`).
  - Found while landing R1564 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1216 A store through a pointer read by its handle does not
  destroy the value it replaces**

  Problem: `var q = Pointer(to=b)`, `q = Pointer(to=b)`, then `q[] =
  Inner(2)` prints `after 2` / `del 2`, where the pin prints `del 1` /
  `after 2` / `del 2`.
  - Cause: `elaborate_store_drops`
    (`crates/mojito-analysis/src/analysis/store_drops.rs`) recovers a
    `WriteRef`'s place only from a `MakeRef` handle. A pointer read by
    `UseVar` names no place, and MIR has no destroy-through-handle
    instruction for a frame/slot pointer, as upstream's `Pointer.__setitem__`
    destroys the old pointee before it stores.
  - Probe: `Pointer(to=h.j).unsafe_origin_cast[origin_of(h)]()` bound to
    `r`, then `r[] = Inner(31)`, misses the pin's `del 30`.
  - Probe: a destructured pointer (`var (p, x) = (Pointer(to=a), 1)`) then
    `p[] = v` over a droppable pointee.
  - Probe: `t[0] = Inner(62)` over `var t = (Inner(60), Inner(61))`
    writes through the reference the inlined `Tuple.__getitem__` returns
    and never destroys `Inner(60)`, where the pin prints `del 60` before
    the next `print`.
  - Probe: `b[0] = P(5)` over a struct whose `__getitem__(ref self, i:
    Int) -> ref[self.v] P` has no `__setitem__`, and `ref r = b[0]` then
    `r = P(5)`, never destroy `P(1)`, where the pin prints `del 1` at the
    store.
  - Found while landing R352 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R355 A view element moved by `Tuple.reverse` or `Tuple.concat`
  loses its loan on the storage it views**

  Problem: `var t = Tuple(StringSpan(s), 1)` then `var r = t.reverse()` and
  `print(r[0], r[1])` stops in the VM with "use after Pointer deallocation",
  where the pin prints `1 hello`.
  - The result carries no loan on `s`, so `s` is destroyed at its own last
    use, before the result is read.
  - The transforms write their named result through pointers, which records
    no origin for the caller to keep.
  - Before the transforms were declared in Mojo the same program was
    rejected with a type mismatch on the result.
  - Reading `t[0]` without a transform prints.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R535 A `ref` field of a list element prints its handle inside `print`**

  Problem: over `l = [RefOne(a)]`, where `RefOne` holds `var value: ref[o] Int`, `print(l[0].value)` prints `<ref 1:0>` instead of `4`.
  - The same happens for a `ref` field of a `Tuple` field's element.
  - `var z = l[0].value` binds, but `print(z)` then prints `None`.
  - Inside the kept direct-`ref`-field extension, so its fixture goes under `assets/extensions/ok/`.
  - Found while planning R529 (2026-10-09).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1295 A partly built `self` dies as one value, where upstream
  tracks each of its fields**

  Problem: a raising initializer that sets a field on only some paths, or
  sets one field before an `if` that sets the next, destroys its partly
  built `self` at other points than the pin, or leaks a field.
  - Drop elaboration ends an initializer's receiver as one variable
    (`drops.rs`), so it dies after its last field store, and
    `refine_receiver_drops` destroys what it holds there.
  - Upstream's `DestructorInsertion` gives each field of `self` its own
    liveness bit beside the full-object bit, so a field no `return` reads
    dies at its own store (`CheckLifetimes.cpp`, where
    `selfPartiallyInitializedAttrName` marks a raise from a partly built
    `self`).
  - A field set on some paths only is maybe initialized where the receiver
    dies, and `refine_receiver_drops` leaves it alone.
  - Native drops carry no run-time flag (R1312), so the receiver's fields
    must die as statically as upstream's per-field bits decide them.
  - Probe: `if c: self.a = Tracer("a")`, then `self.b = Tracer("b")`,
    `print("stored")`, `raise`. The pin prints `del a`, `del b`, `stored`
    for `c = True`, and Mojito leaks `a`.
  - Probe: `self.b = Tracer("b")`, `print("b stored")`, then
    `if c: self.a = Tracer("a")`, `print("a stored")`, then `raise`. The
    pin prints `del b` before `b stored` and `del a` before `a stored`;
    Mojito prints both after `b stored`, `a` first.
  - Probe: `self.item = Inner(40)`, `print("first")`, `self.item =
    Inner(41)` in an initializer. The pin prints `del 40` before `first`,
    the first store dead on arrival; Mojito prints it at the second store.
    The drop walk's bits (`DropValues`) leave the receiver out, so
    `elaborate_store_drops` still destroys it at the store.
  - Found while landing R956 (2026-10-23).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1296 Parameters dying beside locals drop after them, where the pin
  drops them first**

  Problem: `def g(var p: T, var q: T)` with locals `l1` and `l2`, all last
  read by one `print`, prints `del l2`, `del l1`, `del q`, `del p`, and the
  pin prints `del q`, `del p`, `del l2`, `del l1`.
  - The same order shows at a raising arm's entry and at an early
    `return`.
  - `order_deaths` in `drops.rs` sorts a death point's variables by
    descending `VarId`, which puts the parameters last.
  - An initializer's `out self` already drops first, as the pin does.
  - Read upstream's order in `DestructorInsertion` (`CheckLifetimes.cpp`)
    before changing the sort.
  - Found while landing R956 (2026-10-23).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1289 A closed `comptime if`'s dead arm keeps values alive through
  drop elaboration**

  Upstream's `LowerSemanticCF` marks a constant-dead `comptime if` arm
  `unreachable`, and erases the code after a live arm that never falls
  through, before `CheckLifetimes` runs; Mojito keeps both until
  `canonicalize` folds the op, after drop elaboration.
  - Probe: `var b = Thing("b")`, `print("start")`, then `comptime if FLAG:
    return` and `print("after", b.s)`, over a module `comptime FLAG =
    True`: the pin prints `del b`, `start`, and Mojito `start`, `del b`.
    A `raise Error("boom")` in the arm differs the same way.
  - Lever: decide a closed condition before the ownership checks, as
    `markRegionDeadDueToConstantCond` does, the way `hir::Lower` already
    erases the statements after a `return` or `raise`
    (`CheckedTables::live_statements`).
  - Found while landing R906 (2026-10-22).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1497 An associated projection's abandonment is not asked of its
  bound**

  Problem: `_ = s.get()` in `def g[T: Source](mut s: T)`, where `Source`
  declares `comptime Element: Movable` and `def get(mut self) ->
  Self.Element`, runs in Mojito, where the pin rejects it:
  "'(expression temporary)' abandoned without being explicitly destroyed:
  unhandled explicitly destroyed type 'AnyType'".
  - `Checker::destruction_verdict` (`checker/traits.rs`) answers a type
    parameter and an opaque element, not a `TyKind::Assoc` rooted at one,
    which upstream's `getDestructorForType` refines and judges as an
    archetype.
  - Asking it today breaks the bundled `paramfor_*` stubs
    (`std/builtin/_stubs.mojo`), whose `var elem = result.__next__()` is
    `Deinitable` at the pin only through `where
    conforms_to(IteratorType.Element, Movable & Deinitable)`, which needs R792.
  - It also breaks generic borrowed loops while the bundled `Iterator` does
    not refine `Deinitable` (R669) and `Iterable` declares an `Element`
    upstream does not.
  - The lever: an `Assoc` arm in `destruction_verdict` whose root base is an
    own parameter (`is_own_parameter`), judged by `archetype_verdict`.
  - Probe: `x.first() > y.first()` in `def bigger[C: Holder](x: C, y: C)`,
    for `comptime Elem: Comparable & ImplicitlyCopyable` in `trait Holder`,
    runs in Mojito, where the pin rejects both operand temporaries
    ('Comparable').
  - Probe: `var e = s.get()` over the same `Source` is accepted, where the
    pin reports `'e'` abandoned ('AnyType').
  - Probe: `for x in items` in `def total[C: Iterable](items: C)` runs,
    where the pin rejects the `__next__` result at `items` ('AnyType').
  - Probe: `moving_an_iterable_while_its_generic_iterator_lives_rejects`
    (`tests/vm_test.rs`) discards an `Element: Movable` with `_ =
    it.__next__()`; give it a non-discarding read when this lands.
  - Depends on R792, R669.
  - Model: Opus, Not Planned.

- [ ] **R1500 An owned capture is a closure-local snapshot, where
  upstream's is a field of the storage its body takes**

  A `{var s}` or `{s^}` capture is a field of the closure's storage, which
  upstream's body takes as its first argument: the body may write it, and a
  `^` out of it must be refilled before return ("'(positional-only argument
  # 0).s' is uninitialized at return from this function").
  - Mojito binds an owned capture as an immutable local of the body
    (`check_capture_access`, `checker/scopes.rs`), so a write is rejected.
  - `transfer_verdict` admits `s^` out of an owned capture as a place
    move, as upstream does, but MIR then fails to verify ("drop.var of slot
    ... that some path reaching it does not hold").
  - Probe: `def inner() {var s} -> String:` whose body runs `var r = s^`,
    `s = String("b")`, `return r^` prints `a` at the pin; Mojito says
    "expression must be mutable in assignment".
  - Probe: the same body with `return s^` alone (also under `{s^}`): the
    pin reports the uninitialized storage field, Mojito fails MIR
    verification.
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
  - The builtin owned protocol it keeps runs over the bundled
    `VariadicList`, narrowing its span past each element it moves out:
    `checker/iteration.rs::iteration_protocol`, the VM's
    `variadic_list_span`, and Pliron's `advance_variadic_list`; upstream's
    `consume_elements(deinit self, elt_handler)` is not declared yet.
  - Probe: the owned loop normalizes the list in place (`iter.init` with no
    `prepare`), its slot the backend's cursor, where every nominal
    `__iter__` makes an iterator slot of its own type.
  - Pinned by `conformance/probes/variadic_pack_owned_iteration.mojo`.
  - Since R1306 the in-place slot's drop is the library
    `VariadicList.__deinit__` call on the narrowed list.
  - Probe: a list display moves each element twice, out of the pack and
    into the list, where upstream's `consume_elements` moves it once:
    `var a = [mk(1), mk(2)]; print(a[0].n)` over an `M` whose move
    initializer adds 1000 prints `1001` at the pin, `2001` in Mojito.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1613 A list display of elements that are not `Deinitable` fails to
  elaborate**

  `var conns: List[Conn] = [Conn(1), Conn(2)]` over an `@explicit_destroy`
  `Conn` fails with "function instantiation of `VariadicList.__deinit__`
  failed", where the pin runs it.
  - The bundled `List` and `Array` literal initializers move the elements
    out with `for var value in values^` (R32). The narrowed list is then
    destroyed by `VariadicList.__deinit__`, whose `comptime assert
    conforms_to(Self.element_type, Deinitable)` fails.
  - Upstream's initializers hand the list to `values^.consume_elements(
    init_elt)`, a capture-list closure writing each element in place, so the
    destructor is never instantiated.
  - Port `VariadicList.consume_elements` and `_annihilate`, then both
    initializers. An `F: def(idx: Int, var elt: T)` binder stands in for
    upstream's `Some[def(...)]` until R813.
  - Since dcbecf4e every list display calls the literal initializer, so
    this used to work through the compiler-built display.
  - Pinned by `conformance/probes/list_display_linear_elements.mojo`.
  - Depends on R1614 (the closure must package with the library).
  - Model: Opus, Not Planned.

- [ ] **R740 A list display runs the move initializer three times per
  element**

  Problem: `[a^]`, over an `M` whose move initializer adds 1000, prints
  `l 3001`, where the pin prints `l 1001`.
  - `[M(copy=a)]` at `a`'s last use prints `l 3001`, where the pin prints
    `l 2001`.
  - The bundled `List` literal initializer moves each element out of its
    `var *values` pack (R32), where upstream consumes the pack with
    `consume_elements`.
  - Found while landing R730 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1232 A sequence store's nested tuple element followed by a `try`
  that redeclares its leaves fails MIR verification**

  Problem: `[(ref a, ref b), _] = t` inside an `if`, then
  `try: [ref a, ref b] = s`, fails with "drop.var of slot ...
  ('$elem...') that some path reaching it does not hold", where the pin prints
  `1 2` and `5 6`.
  - The nested tuple element is read into a hidden `$elem<id>`
    (`read_into_hidden` in `checker/destructure.rs`) whose drop is
    elaborated on a path the `try` region's cleanup already ended.
  - Probe: `conformance/probes/sequence_nested_ref_redeclared_in_try.mojo`.
  - Found while planning R1138 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1160 An element store while a contiguous `List` slice is live is
  rejected**

  Problem: `var v = xs[:]; xs[0] = 9; print(v)` is rejected ("access to
  'xs' conflicts with live reference 'v'"), where the pin prints
  `[9, 1, 2, 3, 4]`.
  - The slice is a `Span` over `origin_of(xs)._get_owned_interior[
    "element"]`, and an element store does not move the elements.
  - Distinct from R1155, which is `Span(list)` followed by a structural
    mutation.
  - Probe: an element read beside a live mutable view is rejected too.
    `var s = Span(l); s[0] = 7; print(l[0], len(s))` reports "access to
    'l' conflicts with live reference 's'", where the pin prints `7 3`
    (found while landing R1725, 2026-10-30).
  - Probe: `conformance/probes/list_slice_span_element_store.mojo`.
  - Found while landing R192 (2026-10-19).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1240 A getter on a field of a read-only `match` anchor conflicts
  with the anchor**

  Problem: `match w: case W(l=[a, b])` and `case W(o=.Some(x))` over a
  place `w` fail with "access to 'w.l' conflicts with live reference
  '$match<id>'", where the pin runs both.
  - No case binds by `ref` or `as`, so the anchor `ref $match<id> = w` is
    made immutable after its check (`make_reference_immutable`,
    `checker/patterns.rs`); the `ref self` `List.__getitem__` on
    `$match<id>.l` then counts as a write through it, since
    `loan_accesses`' `access_for_convention` (`analysis/loans.rs`) treats
    every `ref` receiver as a write, and `check_loans_at` rejects a write
    through an immutable reference.
  - The anchor's loan itself excludes nothing since R1082, so this
    through-reference rule is the whole rejection.
  - With a `ref` binding in any case the anchor stays mutable and the same
    reads run, and a getter on the anchor itself (`$match<id>[0]`) runs.
  - Probe: `conformance/probes/match_field_getter_read_anchor.mojo`.
  - Found while landing R1133 (2026-10-20).
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
  - Probe: a list display converted at a `ref` parameter is not
    materialized either. `h([1, 2, 3])` over `def h(ref a: List[Int])`,
    and `Span([1, 2, 3]).__len__()` through `Span`'s `List` constructor
    (which the pin selects for a display), fail the verifier ("write-back
    parameter 0 of 'h' has no caller place"). `var s = Span([1, 2, 3])`
    reports "no field 'size'", and `len(Span([1, 2, 3]))` says `Span[Int]`
    is not `SizedRaising`. The pin prints `3` for each (found while
    landing R1725, 2026-10-30).
  - It materializes through the same fallback divergence R169
    (`ref-binding-register-value`) wants narrowed, so the two answers must
    agree.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R406 A subscript of a call result handed to `print` stops at run
  time**

  Problem: `print(mk()[0])` over `def mk() -> Tuple[Int, Int]` (or `->
  List[Int]`) prints `1` at the pin and stops in Mojito with "reference
  binding to a non-place expression".
  - `var x = mk()[0]` and `print(mk()[0] + 1)` run, so only a subscript
    result forwarded by reference to `print` reaches the failure.
  - A tuple display subscripted as a temporary (`(1, 2)[0]`) runs: the
    display's `ConstructCollection` carries its materialized owner.
  - Found while landing R367 (2026-10-06).
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
  - Probe: `var l2 = l.copy()` over `l: List[Span[D, origin_of(xs)]]`,
    with `D` printing in its destructor, then `l2[0][0].a`, stops the VM
    with "use after Pointer deallocation". The pin and `--backend pliron`
    print the element.
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
  origin argument (`Span.copy()` at `Span[Int, origin_of(xs)]`) records a
  view-result borrow of its receiver, so the copy looks like it aliases what
  it was copied from.
  - In `def g(xs: List[Int])`, `var v = Span(l)` over `l:
    List[Span[Int, origin_of(xs)]]`, then `var w = v[0].copy()`,
    `l.append(Span(xs))`, and a read of `w` is rejected ("access to 'l'
    conflicts with live reference 'w'"). The pin prints `2 3`.
  - `w = l[0].copy()` and `l.extend(l.copy())` are accepted, as at the pin.
  - At a mutable origin (`var xs` in `main`), the pin rejects
    `l.extend(l.copy())` itself ("aliasing values passed mutably"), and so
    does Mojito.
  - The rule is `record_view_result` in `method_calls/call_contract.rs`. It
    lends the receiver for any loan-carrying struct result of a
    non-consuming method.
  - Dropping the borrow when the declared return names only the struct's own
    origin binders was tried. It lost the only link from `c.get()`'s result
    to the loans `c` carries, which R87 has to supply first.
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

- [ ] **R674 A parameter annotation cannot name an earlier parameter's
  origin**

  Problem: `def f[T: Copyable](items: T, s: Span[Int, origin_of(items)]) ->
  Int` is rejected with "Undefined variable 'items'", where the pin accepts
  it.
  - The same error rejects a named `out` result annotation
    (`out r: Span[Int, origin_of(items)]`), which the pin rejects for a
    different reason: "cannot return 'items's origin, because it might
    expand to a RegisterPassable type".
  - Parameter annotations resolve before the parameters are in scope.
  - Once the names resolve, `Checker::check_result_origins`
    (`crates/mojito-checker/src/checker/origins/sig.rs`) already judges the
    named result, so the `out` case should then report the pin's text.
  - Found while landing R668 (2026-10-13).
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

- [ ] **R1156 An argument bound to an unbound-origin parameter carries no
  origin into argument exclusivity**

  Problem: `f(Span(xs), Span(xs))` over `def f(a: Span[Int, _], b:
  Span[Int, _])` and a `var xs` runs, where the pin rejects it with
  "aliasing values passed mutably to 'a' argument and passed mutably to
  'b' argument in 'f' call".
  - The same call is judged when the parameters spell a bound origin
    (`rhs: Box[Self.o]`).
  - `check_argument_origin_exclusivity` records carried places from the
    declared and bound parameter types, and an unbound `_` slot carries
    none; the argument's own type carries the origin.
  - Probe: `conformance/probes/unbound_origin_arguments_alias.mojo`.
  - Probe: `conformance/probes/list_slices_compared_alias.mojo`,
    `xs[0:2] != xs[1:3]` through `Span.__ne__(self, rhs: Span[Self.T, _])`.
  - Probe: `b.any(Span(xs))` over a method `any(self, rhs: Span[Int, _])`
    of a struct holding `Span(xs)`.
  - Found while landing R190 (2026-10-19).
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
    clause fails, so that instance is destroyed without it. Natively its
    `String` leaks.
  - `verify_builtin_conformance` (`checker/traits.rs`) answers `Deinitable`
    with `true`. It never compares the `__deinit__`'s `where` clause with
    the conformance's condition, as `method_satisfies_requirement_under`
    does for a declared requirement's availability.
  - The pin reports the unproven clause as "lacking evidence": ask
    `Checker::struct_conformance`, which answers unknown for it.
  - Probe:
    `conformance/probes/conditional_deinit_without_conditional_conformance.mojo`.
  - Depends on R1364.
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
  - Probe: `def h[T: AnyType, o: ImmOrigin](r: Pointer[T, o]._mlir_lit_ref)`
    called with `__get_mvalue_as_litref(x)` fails the same way, and R1314's
    `VariadicList.__init__[size, container_origin]` needs both inferred.
  - Found while fixing a call-result pointer's dereference (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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
  - Probe: `assets/ownership_error/list_slice_used_after_clear.mojo`, a
    `List` slice used after `xs.clear()`: the pin reports the invalidated
    `xs["element"]`, Mojito the conflict with the whole of `xs`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R302 A linear `Dict.deinit_with` leaks its emptied entries list,
  and the closure that would free it cannot enter the stdlib package**

  Problem: `Dict[Int, Conn].deinit_with` over a linear `Conn` pops every
  entry and leaves `entries`, an empty `List[DictEntry[Int, Conn]]`, to the
  `deinit self` residual teardown, so `--backend pliron` leaks its buffer.
  - That list's `__deinit__` is conditional on a `Deinitable` element, so the
    elaborator rightly mints none and concrete MIR never frees the buffer.
  - The checker accepts that residual destruction, as the pin does.
  - `StringDict.deinit_with` (`stdlib/std/collections/string_dict.mojo`)
    has the same shape.
  - The lever: `def forward(var entry: DictEntry[...]) {elt_handler}:
    entry^.reap_with(elt_handler)`, then `self.entries^.deinit_with(forward)`.
    It runs on the VM and natively and frees the buffer.
  - The stdlib package refuses it. `declared_overloads`
    (`mojito-checked/src/declaration.rs`) counts the closure storage's
    synthesized fieldwise constructor, which the check's
    `OverloadSets::scan` never sees, and a declaration record holds only its
    own struct (`LoweredDeclaration::refuse_unjoinable`, `join`).
  - The fix: leave closure storage out of `declared_overloads`, and let a
    record carry the storage structs nested in its declarations, each named
    by its own directory row.
  - Probe: `assets/ok/dict_insert_linear_capable.mojo`, counted with
    `MOJITO_RT_DBG_POINTERS=1` (14 allocations, 13 frees).
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
    argument stays in its register (`Flatten::operand_expr_c`).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R351 A named `out` result of a type parameter is reported abandoned**

  Problem: `def make[T: Defaultable & Movable](out result: T): result =
  T()` stops with "'result' abandoned without being explicitly destroyed",
  where the pin prints the default value.
  - The result is returned, so it owes no destruction in the callee.
  - A named result of a concrete type runs, on a function and on a method.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R353 A `Pointer` to a tuple element does not bind at the tuple's
  annotated origin**

  Problem: `var p: Pointer[Int, origin_of(t)] = Pointer(to=t[0])` stops with
  "type mismatch for variable 'p'", naming two origins that print alike,
  where the pin prints the element.
  - The pointer's origin is the element's place, which the annotation's
    origin of the whole tuple should admit.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1298 A pointer to an element a runtime index reaches through
  `__getitem__` fails MIR verification**

  Problem: `Pointer(to=l[i])` over a `List[Int]` or a `Span` fails with
  "dynamic element projection requires checked indexed storage, got
  List[Int]", where the pin prints the element.
  - The checker takes the syntactic `l[i]` as an index projection of the
    struct itself rather than the place the selected `__getitem__`'s
    reference names; a constant index into compiler-private storage
    (`Tuple`'s `self[i]`) is the only shape that works.
  - Probe: `conformance/probes/pointer_to_runtime_subscript.mojo`.
  - Probe: the bundled `VariadicList.__deinit__` reaches each element
    pointer through its span (`elements[unsafe_offset=i]`) where upstream
    writes `Pointer(to=self[i])`.
  - Found while landing R743 (2026-10-23).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1305 A reference returned through a pointer a call yields fails
  MIR verification**

  Problem: `return self.ptr()[unsafe_offset=i]` from a `-> ref[...] Int`
  method fails with "register has no checked type" for the reference's
  `ref.make`, whose place is rooted at an untyped slot holding the call's
  result, where the pin prints the element.
  - The same pointer bound to a local first (`var p = self.ptr(); return
    p[unsafe_offset=i]`) runs.
  - Probe: `conformance/probes/reference_through_call_result_pointer.mojo`.
  - Probe: the bundled `VariadicList.__getitem__` binds its span's pointer
    before indexing it, where upstream returns
    `self._value.unsafe_ptr()[unsafe_offset=idx][]`.
  - Found while landing R1073 (2026-10-23).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1299 A result origin built from an origin union and
  `unsafe_mut_cast` is rejected**

  Problem: `-> ref[origin_of(Self.origin, self_origin).unsafe_mut_cast[
  Self.mut]()] Int` stops with "unsupported origin contract"
  (`checker/origins/sig.rs`), where the pin prints the element.
  - It is upstream's `VariadicList.__getitem__` result, so the bundled one
    returns `ref[origin_of(self)]`.
  - Probe: `conformance/probes/variadic_list_getitem_origin.mojo`.
  - Found while landing R743 (2026-10-23).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1300 A method's origin parameter bound by `ref[o] self` is not
  the receiver's origin inside the body**

  Problem: `def it[o: ImmOrigin](ref[o] self) -> It[o]: return It[o](0,
  self)`, where `It.__init__` takes `ref[Self.o] src`, is rejected ("expected
  a reference whose origin lies within the supplied 'o' argument, found a
  reference of origin Place(...)"), where the pin prints `0`.
  - The checker gives `self` its place's origin, not the bound parameter.
  - Probe: the bundled `VariadicList.__iter__` takes `ref self` and passes
    `origin_of(self)`, where upstream's is `__iter__[self_origin:
    ImmOrigin](ref[self_origin] self)`.
  - Probe: `def p[origin: Origin, //](ref[origin] self) -> Pointer[Int,
    origin]: return Pointer(to=self.x)` is rejected ("expected
    Pointer[Int, origin#0], found Pointer[Int, ImmOrigin(...)]"), and so
    is a `ref self` method returning `self.ptr()` at `origin_of(self)`.
    The bundled `OwnedPointer.ptr` and `unsafe_ptr` therefore take `ref
    self` with their own bodies, where upstream's take `ref[origin] self`
    and `unsafe_ptr` forwards to `ptr()`.
  - Found while landing R743 (2026-10-23).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1952 `StringSpan`'s canonicalizing constructor
  `__init__(other: StringSpan, out self: StringSpan[ImmOrigin(other.origin)])`
  is not declared**

  `StaticString(P)` over a `StaticString` parameter or local is rejected
  ("expected a reference whose origin lies within the supplied 'origin'
  argument"), where the pin prints it through that `@implicit` constructor.
  - Declaring it verbatim in `std/string.mojo` fails to package: a bare
    `StringSpan` parameter type inside the struct's own declaration is
    "expects 0 type argument(s), got 1", and the result type's
    `ImmOrigin(other.origin)` reads an argument's origin parameter.
  - Probe: `def f[P: StaticString](): print(StaticString(P))`, and
    `comptime assert True, StaticString(P)`; both print or pass at the pin.
  - Found while landing R1881 (2026-11-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R750 A discard of a field transfer `_ = w.m^` is rejected**

  Problem: `var w = W(M(1), M(2)); _ = w.m^` over a destructor-less
  `@fieldwise_init struct W(Movable)` reports "field 'w.m' destroyed out of
  the middle of a value", where the pin runs it.
  - The pin destroys the field in place, then the rest of `w`:
    `del 1001`, `del 1002`.
  - The pin rejects `var t = w.m^` with the same message, so only the
    discard is accepted.
  - MIR `transfer` lowers a field transfer to `MovePlace` before it reads
    the discard's forwarding fact.
  - Found while landing R748 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R696 A reference argument invalidated by a later argument is
  rejected where the pin runs the call**

  Problem: `two(d["k"], d.pop("k"))` and `(d["k"], d.pop("k"))` are
  rejected as a use of an invalidated interior reference `d["value"]`,
  where the pin compiles them and raises `DictKeyError` at run time.
  - The rejection comes from the R685 setup read of a reference result
    (`setup_reference`), which a display joined in R686.
  - `two(lst[0], bl(lst))` over a `List` is rejected at the pin too, so
    the difference is specific to how the pin's `Dict` lookup reads.
  - Found while landing R686 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1212 A dereference of a pointer an ordinary call returns is not a
  place**

  Problem: `bump(idp(p)[])` over `def idp[o: MutOrigin](v: Pointer[Int, o])
  -> Pointer[Int, o]` fails with "write-back parameter 0 of 'bump' has no
  caller place", and `ref r = idp(p)[]` then `r = 11` leaves `x` at `7`,
  where the pin writes through both.
  - The `ref` binding copies silently: `origin_place` has no place for the
    call, so `materialized_reference_actual` materializes the pointee.
  - A pointer formed in place (`Pointer(to=x)[]`, an origin cast, an
    offset) takes a hidden holder slot in `materialize_temporary_holder`;
    an ordinary call's pointer result cannot yet.
  - Cause: MIR's `$mat_r` slot (`materialize_borrow_slot`) loans what
    `aggregate_borrows_unmaterialized` finds, which misses the owner loan
    the read path's `$ptr_tmp_r` slot (`temporary_pointer_deref`)
    establishes. Materializing the call drops that loan, and
    `assets/ok/call_result_pointer_deref_keeps_owner.mojo` prints `None`.
  - Fix: one slot that carries the pointer's loans for reads and places
    alike.
  - Probe: `return self.ptr()[]` under `-> ref[self.x] Int` also needs
    R1213.
  - Found while landing R679 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1234 `Pointer(to=)` an owned value is rejected instead of
  materializing it**

  Problem: `var p = Pointer(to=make())` over `def make() -> D` is rejected
  with "Pointer(to=...) requires a place expression", where the pin
  materializes the temporary and keeps it alive until the pointer's last
  use (`after`, `3`, `deinit 3`, `end`).
  - A call returning a reference is already a place (`Pointer(to=w.get())`
    runs), through its recorded `ReferenceResult` fact.
  - Cause: the checker's `Pointer(to=)` arm (`checker/operators.rs`)
    requires `origin_place` of the operand, and an rvalue has no owner for
    the pointer's origin to name.
  - Fix: give the rvalue a hidden owner slot whose lifetime the pointer's
    loan extends, as a `ref` binding of a temporary
    (`materialized_reference_actual`) already gets.
  - Probe: `conformance/probes/pointer_to_owned_temporary.mojo`.
  - Probe: `Pointer(to=v[1])` over a `SIMD`, and `Pointer(to=s[0])` over a
    user value `__getitem__`, are rejected the same way, where the pin
    points at the materialized result (`2`).
  - Found while landing R1132 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1665 A `Pointer` formed over a place has no address on the VM**

  `Int(Pointer(to=x)) != 0` prints `True` at the pin and natively; the VM
  fails with "vm: SIMD[DType.int, 1] has no method '__int__'".
  - Cause: the VM holds a place-formed pointer as a `Value::Ref` (frame,
    slot, projection), which has no integer form beside
    `runtime::pointer_address`'s for an allocation's `Value::Pointer`.
  - The method dispatch (`VmBackend::method_call`, `vm/invoke.rs`) also
    chases a `Ref` receiver to its referent, so `Pointer.__int__` reaches
    the pointee instead of the pointer.
  - Fix: give a `Ref` a provenance-unique synthetic address, and dispatch
    a `Pointer`-typed receiver's own methods before chasing it.
  - Probe: `p.__int__()`, `Int(p)`, `UInt(p)`, and `def addr[T:
    Intable](v: T) -> Int: return Int(v)` over `var p = Pointer(to=x)`.
  - Found while landing R1399 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1213 A method's pointer result over a field of `self` is rejected
  at every call**

  Problem: `def ptr(ref self) -> Pointer[Int, origin_of(self.x)]` then
  `var p = s.ptr()` fails with "Undefined variable 'self'", where the pin
  prints `p[]`.
  - `-> Pointer[Int, origin_of(self)]` runs.
  - Cause: `PointerOrigin::SelfPlace` carries interior tags but no field
    path, so `pointer_origin_expr`
    (`crates/mojito-checker/src/checker/type_resolution.rs`) resolves
    `origin_of(self.x)` as a concrete place, which a call site cannot
    rebase onto its receiver.
  - Found while landing R679 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1214 Assigning to a method's reference result over heap storage
  loses the write**

  Problem: `s.get() = 7` over `def get(mut self) -> ref[self] Int`, whose
  body returns `p[0]` from `var p =
  self.data.unsafe_origin_cast[origin_of(self)]()` (a heap pointer field),
  leaves the heap cell at `42`, where the pin prints `7`.
  - `ref r = s.get()` then `r = 7` writes through, as does `at(p) = 7` over
    a free function returning `q[0]` from a local copy of a heap pointer.
  - The same loss happens under `-> ref[MutUntrackedOrigin] Int` with `var
    p = self.data`.
  - Likely cause: the returned handle points into the callee's local
    pointer slot;
    `canonical_reference_parts`
    (`crates/mojito-vm/src/backend/vm/references.rs`) stops at a pointer
    value rather than re-rooting at its pointee, and the
    call-as-assignment-target path writes through it after the frame is
    gone.
  - Found while landing R679 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R678 A callee's returned reference register is typed mutable and
  untracked**

  Problem: in `def field(w: W) -> ref[origin_of(w.t)] Int: return w.t`, MIR
  types the `ref.make` register as `ref { Int, origin_untracked { mutable:
  true }, mutable }`, where the declared result is an immutable reference
  rooted at `w.t`.
  - Callers are unaffected, because the call site re-derives the origin
    from the contract.
  - It is a verifier-facing inaccuracy in `ref.make` typing.
  - Found while landing R672 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1323 A pointer cast to an unsafe any-origin keeps no owner alive**

  `UnsafePointer(to=x).unsafe_origin_cast[MutUnsafeAnyOrigin]()` stored in a
  struct and read after `x`'s last use reads a destroyed `x`: the VM traps
  with "field access on non-struct <moved>", where the pin keeps every owner
  alive to the end of the scope and prints its value.
  - Natively the same program reads freed storage.
  - Probe: the `rebinding_reference_aggregate_releases_each_owner_generation_once`
    program of `tests/drops_test.rs` with each `Borrowed(UnsafePointer(to=x))`
    spelled through that cast.
  - Found in the 2026-10-24 overnight-gate triage.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1506 A walrus yields an owned value where upstream yields its
  destination borrowed**

  Upstream's `BinOpNode::emitWalrus` yields a memory lvalue's destination as
  a borrowed value after the store, and a setter-stored target's right-hand
  side. Mojito's checker types every walrus result as an owned value, and
  MIR yields a register.
  - Over a name, the walrus binds the value's register to the slot and
    yields the same register. `print(s := String("b"))` over a `String`
    fails MIR verification ("call reads owned register … which some path
    reaching it has ended"), where the pin prints `b`.
  - Over a place, MIR yields a copy of the stored value taken before the
    store (`Flatten::lower_walrus_store`). A copy constructor runs once more
    than at the pin.
  - So the check rejects a walrus over a place of a non-`Copyable` type
    (`(h.n := N(5)).v` over a `Movable` `N`), which the pin runs.
  - `var d = (h.c := C(6))` over a `Copyable`, non-`ImplicitlyCopyable` `C`
    runs, where the pin rejects the implicit copy of the borrowed result.
  - The lever: the walrus result is read as its target is, a place the
    consumer copies or borrows, rather than a fresh temporary.
  - Found while landing R1375 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Catch Up To Current Mojo: Calls, Overloads, Conversions, And Traits

Track: `calls`.

Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

- [ ] **R1172 Overloads that differ only in compile-time parameter types
  collapse to the last one**

  Problem: `g[DType.int8]()` over `def g[d: DType]() -> Int: return 1`
  beside `def g[n: Int]() -> Int: return 2` prints `2`, where the pin
  prints `1`.
  - A free function's lowered name spells its run-time signature only
    (`lowered_def_name` in `crates/mojito-symbol/src/symbol.rs`), so the two
    declarations share one symbol and the later replaces the earlier.
  - A trait-valued overload (`g[Tr: type_of(AnyType)]` beside
    `g[n: Int]`) shares it: `g[Copyable]()` prints `2`, the pin `1`.
  - Upstream selects among such overloads by the bracket arguments'
    types, as it selects among run-time signatures.
  - Probe: `def f[w: W]()` beside `def f[n: Int]()`, `W` with an
    `@implicit __init__(out self, value: Int, /)` storing `value * 2`, the
    first returning `w.v` and the second `n + 100`: `f[W(3)]()` and
    `f[n=4]()` fail MIR verification on the shared `f$ov$`. The pin prints
    `6 104`. `f[3]()` is ambiguous at both.
  - Overloads that differ only in their `where` clause share the cause.
    Probe: `def p[n: Int]() -> Int where n >= 0` returning `1` beside
    `... where n < 0` returning `2`: `print(p[3](), p[-3]())` prints `2 2`,
    where the pin prints `1 2`.
  - Probe: the same pair as methods of one struct is rejected ("'p' is
    already declared in this scope"), since `same_method_shape`
    (`checker/traits.rs`) compares run-time signatures only.
  - Upstream's `IntLiteral.__pow__` is such a pair (R1821).
  - Found while planning R1166 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1853 A method's signature cannot call a method its struct declares
  later**

  `def neg(self) -> type_of(self.sub(self))` declared before `sub` is
  "type 'W[n]' has no method 'sub'", where the pin resolves it.
  - `register_struct_method_signatures` (`checker/traits.rs`) types each
    signature in declaration order and registers it after, so a signature
    sees only the methods above it.
  - Upstream resolves a signature's calls against the whole struct.
  - Probe: `struct W[n: Int]` with `neg` as above before `def sub(self, o:
    W[_]) -> W[Self.n - o.n + 7]`: `print(W[3]().neg().n)` prints `7` at
    the pin.
  - Landing it ports upstream's `IntLiteral.__neg__` (`type_of(0 -
    self)`), `__invert__`, and `__ceildiv__`, and `FloatLiteral.__neg__`
    and `__ceildiv__`, which read `__sub__`, `__xor__`, `__floordiv__`, and
    `__mul__` declared below them. Each fails the bundled library's
    packaging today.
  - It then deletes the prefix literal arms of `Checker::infer_prefix`
    (`checker/operators.rs`): `-N` and `~N` become the declared dunders.
  - Probe: `print(-4, ~4, (7).__ceildiv__(2), (2.5).__neg__())` prints `-4
    -5 4 -2.5` at the pin.
  - Probe: `comptime X = 12`, then `print(X.__neg__())`, prints `-12` at
    the pin; the unannotated constant is the `IntLiteral[12]`, so the call
    reaches the unported `IntLiteral.__neg__`.
  - Found while landing R1753 (2026-11-02).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1854 A signature's call to a method whose `_` binder shares a
  caller binder's spelling captures it**

  `def radd(self, rhs: W[_]) -> type_of(rhs.add(self))` over `def add(self,
  rhs: W[_]) -> W[Self.n * 10 + rhs.n]` gives `W[1]().radd(W[2]()).n ==
  11`, where the pin prints `21`.
  - The same signature with the caller's parameter renamed (`other:
    W[_]`) prints `21`, so the callee's auto-parameterized binder
    (`auto_binder_name`, `comptime/auto_parameterize.rs`) and the
    caller's are substituted as one.
  - Instantiating the callee's return type must bind its own binders
    simultaneously, never through a caller binder of the same spelling.
  - Landing it ports upstream's reflected `FloatLiteral` dunders
    (`__radd__`, `__rsub__`, `__rmul__`, `__rtruediv__`, each
    `type_of(rhs OP self)`), which give `2 + 2.5 == 5.0` today, so `2 +
    2.5` becomes `FloatLiteral.__radd__`, and deletes the mixed-family
    case of `Checker::literal_rule` (`checker/operators.rs`).
  - Probe: `print(2 + 2.5, 1 / 2.0, 3 - 0.5, (2.5).__radd__(1.0))` prints
    `4.5 0.5 2.5 3.5` at the pin.
  - Probe: `print(7 // 2.5)` and `print(7 % 2.5)` fail at the pin with
    "constraint failed: the SIMD type must be floating point", since no
    reflected `//` or `%` takes an `IntLiteral` and both operands
    materialize to `Int`; Mojito's literal rule types them
    `FloatLiteral[2]` and prints `2.0`.
  - Found while landing R1753 (2026-11-02).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1625 A written-back subscript is loaded once where an augmented
  store loads it twice**

  Problem: an augmented store whose place is rooted in a value
  `__getitem__` with a `__setitem__` calls the getter once, before its
  right-hand side, where the pin calls it after the right-hand side, once
  for the load and again for the store.
  - Cause: MIR flattens a place once per statement, so the write-back
    temporary `materialize_write_back_place` (`mir/write_back.rs`) makes
    serves both the read and the write.
  - Upstream's `SubscriptDLValue` reloads its base at every access: the
    load of the augmented place and its store each call the getter.
  - Lever: split the write-back into its once-evaluated operands and a
    getter call the augmented paths (the projected-place arm of
    `lower_stmt`'s `AugAssign`, `lower_augmented_index`,
    `lower_augmented_in_place`) emit once for the load and once for the
    store.
  - Probe: `s[idx(0)].x += rhs(4)`, `s` with a printing value getter and
    setter, prints `idx`, `rhs`, `get 0`, `get 0`, `set 0` at the pin,
    while Mojito prints `idx`, `get 0`, `rhs`, `set 0`.
  - Probe: `o[0][0] += 3`, both levels a value getter and a setter, prints
    `get`, `get`, `inner set`, `set` at the pin, while Mojito prints one
    `get`.
  - Found while landing R1503 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1626 A store through a projected place evaluates its right-hand
  side before the place's operands**

  Problem: `ps[idx(0)].x = rhs(5)` over a `List[P]` prints `rhs` before
  `idx`, while the pin evaluates the target's operands first and prints
  `idx`, `rhs`.
  - Cause: `lower_set_place` (`mir/lower_stmt.rs`) lowers the value
    (`lower_assignment_value`) before it flattens the place, while a
    nominal subscript store (`lower_subscript_setter`) already evaluates
    its receiver and indices first.
  - Upstream's `emitAssign` forms the target's lvalue before it emits the
    right-hand side, then stores.
  - Probe: the same store through a value getter with a setter,
    `s[idx(0)].x = rhs(5)`, prints `rhs`, `idx`, `get 0`, `set 0` where the
    pin prints `idx`, `rhs`, `get 0`, `set 0`.
  - Found while landing R1503 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1627 A sliced value-getter subscript is not written back as a
  mutable place**

  Problem: a subscript with a slice argument, over a value `__getitem__`
  and a `__setitem__`, used as a mutable place is rejected ("a sliced value
  '__getitem__' result used as a mutable place is written back through
  '__setitem__'"), where the pin loads it, modifies it, and stores it back.
  - Cause: the checker's `record_subscript_write_back`
    (`checker/indexing.rs`) and `materialize_write_back_place`
    (`mir/write_back.rs`) take index arguments only; a slice literal is
    rebuilt per accessor, as the augmented store's `slice_set_argument`
    does.
  - Probe: `s[0:2].bump()` with `__getitem__(self, sl: Slice) -> W` and
    `__setitem__(mut self, sl: Slice, value: W)`.
  - Probe: `bump(s[1:])`, a `mut` argument over the same accessors.
  - Probe: `g[0, 1:].x = 5` over a two-argument getter and setter.
  - Found while landing R1503 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1628 A temporary passed to a `mut` parameter is accepted**

  Problem: `bump(five())` over `def bump(mut x: Int)` passes the check and
  fails MIR verification ("write-back parameter 0 of 'bump' has no caller
  place"), where the pin rejects it ("value passed to mutable argument 'x'
  must be mutable").
  - Cause: a call's `mut` arguments are checked for a `^` transfer
    (`reject_transfer_into_mutable`, `checker/places.rs`) and, since
    R1503, for a value subscript (`bind_mutable_subscript_arguments`), but
    not for being a place at all.
  - Lever: require `designates_place` (or a mutable reference result) of
    every `mut` argument beside those two checks.
  - Probe: at compile time, `g[bc(String("ab"))]()` over `def bc(mut s:
    String) -> Int` fails the verifier in the lifted thunk ("write-back
    parameter 0 of 'bc' has no caller place"), where the pin rejects it
    the same way. No compile-time application takes a `mut` parameter.
  - Found while landing R1503 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1962 A merge of two pointers, spans, or string slices of
  different origins is rejected, where upstream's `__merge_with__` unions
  them**

  Problem: `Pointer(to=a) if c else Pointer(to=b)` stops with "value of
  type 'Pointer[Int, origin@…]' is not compatible with value of type
  'Pointer[Int, origin@…]'", where the pin prints `1`.
  - Cause: `Checker::common_type` (`checker/operators.rs`) models
    upstream's `findCommonType` without its third step, a type's
    `__merge_with__[other_type]` conversion, and its second, the
    meta-type bound merge.
  - Upstream's `Pointer`, `Span`, and `StringSlice` declare
    `__merge_with__`, whose result origin is `origin_of(Self.origin,
    other_type.origin)`; the bundled ones declare none.
  - Probe: `StringSlice(s1) if c else StringSlice(s2)` over two `String`
    locals fails the same way ("'StringSpan' is not compatible with
    'StringSpan'"); the pin prints `x`.
  - Probe: `Span(l1) if c else Span(l2)` over two lists fails the same
    way; the pin prints `2`.
  - Found while landing R1848 (2026-11-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R2020 A module `comptime` constant annotated `Int` over a
  `SIMDLength` constant reads as the `SIMDLength`**

  `comptime W: SIMDLength = 4` then `comptime N: Int = W` fails at each
  read of `N`, where the pin binds the lane count.
  - `fold_module_constants` (`checker/constraints.rs`) folds both to the
    literal `4`, dropping `W`'s `SIMDLength` annotation, so `N` checks as
    an `Int`.
  - A read of `N` still lowers its initializer `W`, a `SIMDLength`, not the
    lane count the `Int` slot binds through `Checker::simd_length_lanes`,
    as R1687's local binding now does.
  - Probe: `print(N)` stops with "MIR function 'main' block 0: binding of
    SIMDLength to a slot of type Int"; the pin prints `4`.
  - Probe: `print(N * 2)` stops with "unsupported monomorphizing
    `SIMD.__mul__`: expected `SIMD[dtype, length]`, found `SIMDLength`";
    the pin prints `8`.
  - Found while landing R1687 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1504 A slice or multi-index store through a getter's mutable
  reference is rejected**

  Problem: `s[1, 2] = 5` on a struct whose only accessor is `__getitem__(ref
  self, a: Int, b: Int) -> ref [self.n] Int` prints `5` at the pin, while
  Mojito rejects it ("a slice or multi-index store through '__getitem__''s
  reference result is not supported").
  - Upstream's `emitGetterSetterAccess` emits the getter call whenever there
    is no setter, whatever the subscript's shape, and the store writes
    through its reference.
  - `check_subscript_reference_assignment` (`checker/indexing.rs`) and
    `lower_subscript_reference_set` (`mir/lower_stmt.rs`) take a single
    index only; the augmented store's `lower_augmented_slice` and
    `lower_augmented_multi_index` already lower the getter's handle for
    every shape and are the model.
  - Probe: the store above, and `s[1:2] = 5` over a slice getter returning a
    reference.
  - Found while landing R1373 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1294 A temporary iterable takes the borrowed `__iter__` where
  the pin calls its `__iter__(var self)`**

  Problem: `for x in Bag(1)` over a `Bag` declaring both `__iter__(ref
  self)` and `__iter__(var self)` prints from the borrowed one, where the
  pin calls the owned one.
  - Upstream's `emitForStmt` calls `__iter__` on the source as an ordinary
    method call, whose overload resolution ranks `var self` first for an
    rvalue receiver.
  - Mojito's checker picks the iteration mode from the source's spelling,
    owned only for a `^` transfer (`iteration_protocol` in
    `checker/iteration.rs`), and selects among the conventions that mode
    allows.
  - Ordinary methods overloaded on their receiver convention are rejected
    instead ("'get' is already declared in this scope"), where the pin
    runs `Bag(1).get()` through `get(var self)` and `b.get()` through
    `get(ref self)`.
  - Probe: the loop above, each `__iter__` printing its convention.
  - Probe: `Bag(1).get()` and `b.get()` over `get(ref self)` beside
    `get(var self)`.
  - Found while landing R1290 (2026-10-23).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1114 `not <Trait>` opts a struct out of a trait in its conformance
  list**

  Problem: `struct Handle(not Movable)` and
  `struct Example(not Deinitable else "call 'cleanup()'")` fail to parse
  ("Unexpected token Not: Expected a trait name in the conformance list"),
  where the pin reads them as the preferred spelling of `Movable where
  False` and `Deinitable where (False, "call 'cleanup()'")`, which keep
  working.
  - The manual's explicit-destruction page now spells every opt-out this
    way.
  - Lever: `parse_struct`'s conformance list reads `not NAME [else STRING]`
    into the conditional-conformance-false fact it already builds for
    `where False`, message included.
  - `docs/grammar.md` first, then parser tests and an `assets/ok` fixture
    beside the `where False` ones.
  - Probe: the bundled `Allocation` and `ThinAllocation` spell
    `Deinitable where False`, where upstream's declare `not Deinitable`.
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1502 `Pointer` and `StringLiteral` are not `TrivialRegisterPassable`**

  Upstream declares both `TrivialRegisterPassable`, but the modelled
  conformance (`modelled_trivial_register_passable`, `mojito-types`) and
  the checker's `is_trivial_register_passable` answer no for
  `TyKind::Pointer` and a `StringLiteral[v]` application.
  - Probe: `conformance/probes/builtin_trivial_register_passable.mojo`.
    The pin prints `7 a`. Mojito says "type 'Pointer[Int, ...]' for
    parameter 'T' does not conform to trait 'TrivialRegisterPassable'", and
    infers `keep("a")` at `String`.
  - So `transfer_verdict` (`checker/places.rs`) judges `p^` of a pointer a
    place move, and MIR `transfer` (`mir/lower_expr/expr.rs`) still copies
    one out of a `mut` parameter by `owns_droppable_storage`. Once the
    predicate answers yes, delete that fallback so lowering reads the
    verdict alone.
  - Check the predicate's other callers: `fieldwise_conventions` (a pointer
    field's fieldwise parameter becomes `read`), `origins/sig.rs`'s
    `NoMemoryOrigin`, and `patterns.rs`'s `ref` binding.
  - Probe: a thin function and a closure's storage whose captures are
    trivial conform to `TrivialRegisterPassable` (R1584), but
    `is_trivial_register_passable` still answers no for both: `var g =
    f^; print(g(1), f(2))` over `var f = inc` reports "use of
    uninitialized value 'f'", where the pin warns the transfer has no
    effect and prints `2 3`.
  - Found while landing R1367 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1224 A sequence target inside a tuple target is rejected**

  Problem: `var ([a, b], c) = (values.copy(), 3)` is rejected ("a sequence
  target inside a tuple target"), where the pin runs it and prints
  `1 2 3`.
  - The tuple extraction plan (`destructure_elements` in
    `checker/destructure.rs`) stores every leaf as one MIR-lowered plan,
    but a nested sequence target is R1136's checked desugar, a raising
    length check and per-element calls.
  - Upstream's `TupleDLValue` stores each element into its destination in
    order, so the nested list's length check runs before later siblings
    are stored; the plan must sequence its stores around it, or a tuple
    holding a sequence target must desugar whole.
  - R1374 makes each leaf store through its own destructuring lvalue, the
    path a nested sequence target's store takes in place of the one plan.
  - Stop: the shape above runs, a wrong inner length raises before `c` is
    stored, and a non-raising context rejects.
  - Found while planning R1136 (2026-10-20).
  - Depends on R1374.
  - Model: Fable, Not Planned.

- [ ] **R1231 A lambda called in an augmented assignment's value is a
  compiler invariant violation**

  Problem: `total += (lambda -> Int: 1)()` stops with "compiler invariant
  violated: lambda reached inference before scoped checking", where the
  pin prints `1`.
  - The same lambda in a plain assignment (`total = total + (lambda ...)()`)
    runs, so the augmented assignment's check infers its value before the
    statement-root registration that checks a lambda's hidden `def`
    (`Checker::check_lambda`) has run over it.
  - Probe: `conformance/probes/augmented_lambda_value.mojo`.
  - Found while planning R1138 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1242 Dead regions after an always-completing case or condition do
  not warn**

  Problem: the pin warns `unreachable code after match case that always
  completes`, `unreachable code after match that always completes`, and
  `'if' condition always evaluates to 'True'; 'else' branch is
  unreachable`, where Mojito warns nothing.
  - Upstream's `LowerSemanticCF` (`markRegionDeadDueToConstantCond`)
    walks the structured regions, marks a region dead once an earlier case
    always completes or a condition folds, and warns at its first
    statement.
  - Mojito has no pass that reports dead structured regions; only the
    checker's coverage warnings reach `CheckedProgram::warnings`.
  - Probe: `case _ | True:` then `case False:` over a `Bool` warns at the
    `True` alternative; the `else` of `if case (x, y) = (1, 2)` warns after
    the irrefutable condition; `if True: ... else: ...` warns at the
    condition.
  - Probe: `case 1`, then `case 1 if c`, and a repeated tuple case of an
    11-`Bool` product, warn at the second case's residual in its cluster's
    arm.
  - Probe: `conformance/probes/match_cluster_unspelled_members.mojo` warns
    at its third case's body, the second residual of a group whose first
    always matches.
  - The same missing pass leaves a dead region live for the move scan:
    `lowerLITLoop` follows a loop whose body never breaks with
    `hlcf.unreachable`, and `while True` never breaks once its condition's
    else arm is marked dead, so the loop's exit is unreachable.
  - Probe: `conformance/probes/loop_never_breaks_mut_parameter.mojo`, a
    `mut` parameter moved before `while True: pass`, is rejected as
    uninitialized at return, where the pin prints `x` (found while landing
    R875, 2026-10-21).
  - Found while landing R1134 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1222 `== literal` in a case pattern does not warn that `==` is
  redundant**

  Problem: `case == 2:` and `case 1 | == 3:` run without a diagnostic,
  where the pin warns "'==' is redundant when matching a literal; remove
  it" at the `==`.
  - Upstream's `UnaryOpNode::buildCheckList` (`ExprPatterns.cpp`) warns
    when the operand, parentheses stripped, is a `Bool`, integer, float,
    or string literal. `== k` does not warn.
  - The warning applies at any depth: inside a tuple, a sequence element,
    or a `|` alternative.
  - The lever is `compile_pattern`'s `Equal` arm (`checker/patterns.rs`),
    recording a `Warning` as the coverage warnings do.
  - Upstream warns while it builds the case's commands, so the warning
    goes before the match's coverage warnings in `match_warnings`
    (`check_match_cases`).
  - Add the cases to `assets/ok/match_coverage_warnings.mojo` or a sibling
    fixture that pins warnings.
  - Found while planning R1135 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1243 A starred sequence pattern in `comptime match` is rejected**

  Problem: `case [first, *_]` and `case [first, *rest]` over a compile-time
  `List` are rejected ("a starred sequence pattern in 'comptime match'"),
  where the pin binds the star to the subject's slice and prints `1`.
  - The cause is the compile-time mode of `compile_sequence_pattern`
    (`checker/patterns.rs`): a path is its expression over the subject,
    and no compile-time value spells the slice, a `Span[T,
    ComptimeOrigin]` over the compile-time list.
  - The pin fails only where the slice crosses to run time: `len(rest)`
    in the body is "cannot materialize compile-time value of type
    'Span[Int, ComptimeOrigin]' to a runtime value".
  - Lever: spell `Step::SequenceSlice` in compile-time mode, bind the star
    as `comptime rest = subject[1:]`, and reject its runtime read with
    the pin's text.
  - Found while landing R1139 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1246 Bare and `ref` bindings in `|` alternatives are rejected**

  Problem: `case (x, 2) | (1, x)` over `(1, 5)` is rejected ("a bare or
  'ref' binding in a '|' alternative"), where the pin binds `x` as one
  immutable reference to whichever element matched and prints `5`.
  - Only `var` bindings join today (`Join` in `checker/patterns.rs`): the
    first alternative declares, the later ones assign one slot.
  - Upstream's `emitOr` declares each bound name before the alternatives
    and initializes it in every arm (`initOrBindings`), so a reference
    join holds the union of the alternatives' loans.
  - Lever: the runtime-handle representation (`runtime_aliases` in
    `mir/lower_stmt.rs`) in every alternative, since a static alias names
    one place.
  - `docs/features.md`'s `match` row names this entry.
  - Found while landing R1140 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1248 `comptime if case` is a parse error**

  Problem: `comptime if case 3 = n:` inside `def f[n: Int]()` fails to
  parse ("'comptime if case' is not supported; use 'comptime match'"),
  where the pin tests the pattern at compile time and prints `three`.
  - The pin requires a binding to be a compile-time value (`variable
    binding ... in 'comptime if case' must be a compile-time value`).
  - `parse_comptime` (`parser/stmts.rs`) rejects it after parsing the
    whole chain, so the error lands on the token after the statement (the
    next `def`, `7:1`) rather than at the `case`.
  - Lever: `check_comptime_cases`'s compile-time mode with one case per
    branch, as `comptime match` emits `hlcf.comptime.if`.
  - `boolean_condition` (`checker/statements.rs`) names this entry.
  - Found while landing R1140 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1119 `Defaultable` supplies a default `__init__()`**

  Problem: a struct that conforms to `Defaultable` without writing an
  initializer is rejected ("struct 'S' has no constructor"), where the pin
  synthesizes `__init__()` through reflection, default-constructing each
  field, whenever every field is `Defaultable`.
  - Lever: a trait default body for `Defaultable.__init__` over
    `reflect[Self]`'s fields, the way the `Hashable`/`Writable` defaults
    read them.
  - The default body goes in the bundled declaration
    (`stdlib/std/builtin/value.mojo`), whose `__init__` is a requirement
    until then.
  - The synthesized `__init__` must be the member the struct's
    `Defaultable` witness table records, since `T()` at a struct names
    that witness (`MirDeclarations::default_initializer`).
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R503 A call through a callable local reads a same-spelled
  sibling's slot**

  Problem: two sibling `if c:` blocks binding `var l = one` and
  `var l = two`, each followed by `print(l())`, print `1` and `1`, where
  the pin prints `1` and `2`.
  - In MIR the second `call.indirect`'s callee is `var.use` of the first
    `l`'s slot, while its own `var.store` went to a fresh slot: the callee
    slot is picked by spelling, not by the checked binding.
  - The same two arms in a kept `comptime if n == 0` of a generic `def`
    fail with "'None' is not callable", and in a `comptime for` with a
    `comptime if i == 0` arm they print `1 1`.
  - Found while landing R329 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1084 A call of a function value rebound to another function
  rejects, where the pin binds the first function's defaults**

  `var g = first; g = second; print(g())` over `first(r: Owner =
  Owner(String("first")))` and `second(r: Owner = Owner(String("second")))`
  prints `second first` on the pin: the variable's function literal type
  carries the defaults of the function it was declared from. Mojito's
  `FuncTy` carries only `required`, so `mir::fill_omitted_arguments` fills
  such a call only when every definition of the variable holds the same
  function (`mir::callable_targets`), and otherwise the VM and Pliron
  reject the omitted `Evaluate` slot at the call.
  - A literal default of such a value is still bound by the callee, so
    `second`'s own literal default is taken where the pin takes `first`'s.
  - The lever: a function literal type that keeps its declaration's
    default attributes, which the check records on the variable's type and
    the fill reads instead of tracing the register.
  - Found while landing R1077 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1493 An `IsTrivially*` over a symbolic struct is false, not
  unknown**

  `trivial_lifecycle` (`checker/traits.rs`) answers a Bool, and its field
  recursion over a type parameter is `false`, where upstream's
  `IsTrivially*` over a struct whose fields name an archetype stays
  un-foldable and is judged against the scope's assumptions.
  - So `where IsTriviallyDeinitable[Box[T]]`, called from a generic body,
    reports "violated constraint" where the pin says "lacking evidence to
    prove correctness".
  - `constraint_proposition`'s `Trivial` arm (`constraints.rs`) already
    keeps an archetype operand as its `ParamKind::Trivial` atom; only a
    struct operand folds through the Bool.
  - The lever: `trivial_lifecycle` answers a
    `mojito_types::conformance::Conformance`, a field over an archetype
    being unknown, and the `Trivial` arm keeps an unknown struct operand as
    its atom over `conformance_subject`, as the `Conforms` arm does.
  - Probe: `struct Box[T: Copyable & Deinitable](Copyable)` holding a
    `Self.T`, `def g[T: Copyable & Deinitable]() where
    IsTriviallyDeinitable[Box[T]]`, and `g[T]()` from `def f[T: Copyable &
    Deinitable]()`. The pin says "lacking evidence", Mojito "violated
    constraint".
  - Found while landing R1364 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1494 A conformance boundary's premise is a second assumption
  channel**

  A conditional conformance's `where` premise reaches its conformance check
  as an explicit `assumption: Option<&GenericConstraint>` argument
  (`is_copyable_under_assumption`, `conforms_to_under_assumption[_inner]`,
  `eval_constraint_under_assumption`, `verify_builtin_conformance`,
  `ct_member_satisfies`, `struct_copyable_conformance_ok`,
  `struct_implicitly_copyable_conformance_ok`, all `checker/traits.rs`),
  where upstream passes it as `additionalAssumptions` to the one
  `doesConformTo` (`canDischargeConstraintsInScope`, `Constraints.cpp`).
  - The lever: push the premise's facts (`guaranteed_facts`) and residual
    conjuncts as one level of `Checker::assumptions` and ask
    `Checker::conformance`. The `_under_assumption` family goes.
  - Probe: R1412's library struct whose conditional `ImplicitlyCopyable`
    holds only under its premise; expect it to shrink.
  - Found while landing R1364 (2026-10-27).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R792 `conforms_to` takes only a parameter name and one trait**

  Problem: `where conforms_to(IteratorType.Element, Movable & Deinitable)`
  is rejected with "conforms_to requires a parameter name", where the pin
  accepts a projection subject and a trait composition.
  - The bundled `std/builtin/_stubs.mojo` omits upstream's clause on
    `paramfor_has_next` and `paramfor_next_iter` for this reason. Restore
    it when this lands.
  - Probe: `where conforms_to(IterableType.IteratorType[
    origin_of(iterable)].Element, Boolable & Deinitable)`, the clause of
    upstream's `Iterable` overloads of `any` and `all` (R1605), rejects
    with "conforms_to requires a parameter name".
  - The constraint compiler is `compile_generic_constraint`
    (`crates/mojito-checker/src/checker/constraints.rs`).
  - Probe: `comptime if conforms_to(T.E, Writable):` over a projection is
    rejected as "conforms_to takes a type as its first argument"; the pin
    accepts it, as it does the `where` spelling.
  - The bundled `List.__init__` from an iterable
    (`stdlib/std/collections/list.mojo`) omits upstream's `where
    conforms_to(IterableType.IteratorType[origin_of(iterable)].Element,
    Copyable)`. Restore it when this lands.
  - Once a projection subject enters the assumption store,
    `Checker::refined_bound` must refine a projection by a fact stated of
    it, not only by its base's refined bound (`binder_fact` matches a
    binder or a pack today).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R726 A constructor with its own origin parameter matches no
  overload over a struct argument**

  Problem: `C(xs, RefBox(Pointer(to=ys)))` over `def __init__[o:
  Origin[mut=True]](out self, mut xs: List[Int], b: RefBox[o])` prints
  `2` at the pin; Mojito rejects it: "no constructor overload matches
  the supplied arguments".
  - On a generic `C[T]` it fails instead with "expected RefBox, found
    RefBox".
  - The same signature as a free function binds `o` and runs.
  - Until it is fixed, the carried-origin constructor shape
    `C(xs, RefBox(Pointer(to=xs)))`, which the pin rejects as "... 'b'
    argument in 'C' initializer call", has no `assets/type_error/`
    fixture: add it with this fix.
  - Found while planning R721 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R592 A module `comptime` alias read from a module still loading is
  undefined**

  Problem: when a module reads a `comptime` alias of a module that is
  still loading (an import cycle), the alias is undefined, where the pin
  resolves it.
  - Linking binds the name, but the check binds a module constant at its
    statement in the source-order walk. A module function is declared
    ahead of every body; a constant is not.
  - Probe: moving `std/math/__init__.mojo`'s `from std.utils.numerics
    import …` ahead of `.constants`, upstream's position, stops packaging
    with "Undefined variable '__module$std$$plugin$$impl$CurrentPlugin'":
    `std.math.math` reads `CurrentPlugin` while `std._plugin` is loading.
    Restore upstream's order when this lands.
  - Found while landing R551 (2026-10-10).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R942 A `@staticmethod` trait requirement is rejected**

  Problem: `trait T:` declaring `@staticmethod def make() -> Int: ...` is
  valid at the pin, and upstream's std uses it (`UnsafeNicheable`,
  `DevicePassable`), but Mojito rejects it.
  - The parser requires `self` first in a trait method, so a static
    requirement is a parse error, and the checker rejects
    `@staticmethod` on a trait method as unsupported.
  - A requirement needs a static shape in `TraitRequirement`, conformance
    matching against a struct's `@staticmethod`, and `T.make()` dispatch
    over a bound binder.
  - Probe: the bundled `Array`, `Optional`, `OptionalReg`, `Span`, and
    `StaticTuple` omit upstream's `DevicePassable` conformance, a trait
    with `@staticmethod` requirements; `StaticTuple` also omits
    `_to_device_type` and the static `get_type_name`.
  - Found while landing R914 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1928 An instance call in a parameter context does not resolve
  to the receiver type's static overload**

  `comptime for i in range(s.__len__())` over `var s = S[3](7)`, `S` a
  `RegisterPassable` struct declaring both `@staticmethod def __len__()`
  and `def __len__(self)`, is rejected with "cannot use a dynamic value in
  'for' iterator expression", where the pin selects the static overload,
  warns that `s` is unused, and prints `0 1 2`.
  - The pin's rule: in a parameter context, an instance call through a
    dynamic register-passable receiver selects a matching zero-receiver
    `@staticmethod` overload and never evaluates the receiver.
  - Mojito answers it only for the bundled `VariadicPack`'s `__len__()`
    (`Checker::collector_pack`, `intrinsic_receivers.rs`), as the pack's
    length query.
  - Lever: rank the static overload in method resolution when the call
    is checked in a parameter context, then retire the `VariadicPack`
    special case.
  - Found while landing R1921 (2026-11-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1161 `@align(n)` on a struct is rejected**

  Problem: `@align(16)` on a struct is reported as an unsupported
  decorator, where the pin raises the struct's alignment and size
  (`size_of[A]()` prints `16` for one `Int` field).
  - `checker/decorators.rs` validates the argument count and then rejects
    the decorator.
  - Layout alignment is not a declaration fact `mojito-native-core`'s
    layout reads, nor one the VM's size queries see.
  - Probe: `conformance/probes/decorator_align.mojo`.
  - Found while landing R192 (2026-10-19).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1162 `@unavailable` on a method is rejected**

  Problem: `@unavailable` on a struct or trait method is reported as an
  unsupported decorator, where the pin accepts the declaration and rejects
  each use with the decorator's message.
  - The function half landed with R988: `checker/decorators.rs` returns
    the mark through `DecoratorFacts`, `CallableOriginSignature` carries
    it to overload selection, and `CheckedDef::unavailable` keeps the
    body out of lowering. A method needs the same mark on its
    `MethodSig`, beside the overload selection of a method call.
  - Upstream marks `String`'s and `StringSlice`'s `__len__` and
    `UnsafePointer.__bool__` this way.
  - Probe: a nested `@unavailable` `def` is accepted but still lowers its
    `...` body inside its enclosing function (`mir::lower_stmt`).
  - Probe: a `pass` body is accepted, where the pin wants `...`
    ("unexpected function body in @unavailable function declaration, use
    '...'"): the parser reads `...` as `StmtKind::Pass`.
  - Found while landing R192 (2026-10-19).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1163 `@fieldwise_init("implicit")` is rejected**

  Problem: `@fieldwise_init("implicit")` is reported as unsupported, where
  the pin makes the synthesized fieldwise initializer `@implicit`, so
  `var s: S = 3` runs for a one-field struct.
  - The parser's `fieldwise_init` flag carries no implicit bit, and the
    synthesized `__init__` carries no `@implicit` decorator.
  - The lever is the synthesized `S.__init__`
    (`mir::lower_fieldwise_constructor`): mark it implicit, and let the
    implicit-conversion lookup find it as it finds a written `@implicit`
    constructor.
  - Probe: `conformance/probes/fieldwise_init_implicit.mojo`.
  - Found while landing R192 (2026-10-19).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1164 The check emits none of the pin's decorator warnings**

  Problem: the pin warns at each use of a `@deprecated` declaration (with
  its message or its `use=` replacement), and at an `@export` without an
  `abi()` effect, where Mojito prints nothing.
  - The checker's warning channel already carries the match-coverage
    warnings the CLI prints.
  - `checker/decorators.rs` accepts both decorators silently.
  - Probe: `conformance/probes/decorator_warnings.mojo`.
  - Found while landing R192 (2026-10-19).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1165 A trait method's caller-contract decorators are rejected**

  Problem: `@__unsafe_nested_origins_read_only`, and `@implicit` on an
  `__init__` requirement, are accepted on a trait method at the pin, but
  Mojito reports each as unsupported.
  - `checker/decorators.rs` accepts only the inline levels and the
    documentation decorators at the trait-method site.
  - A requirement would need to carry the contract, and conformance would
    need to compare it with the implementing method's.
  - `@staticmethod` there is R942, and `@export` R936.
  - Probe: `conformance/probes/trait_method_decorators.mojo`.
  - Found while landing R192 (2026-10-19).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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
    checker's builtin hashable-leaf arm (`method_calls/resolution.rs`), so the call
    side the rename needs exists; that leaf is still the checker's
    intrinsic, not a real `SIMD.__hash__` body.
  - Upstream also replaced the pointer-and-length `hash()` overload with
    `hash_bytes(ImmSpan[Byte])`, which lands in the same pass.
  - The levers are `stdlib/std/hash/` (the bundled `Hasher` declaration in
    `hasher.mojo`, `_ahash.mojo`, `_fnv1a.mojo`, plus every
    `hasher.update(...)` call site).
  - Depends on nothing.
  - Model: Fable, Planned.

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
  - A `mut` parameter moved before `abort("no")` is rejected as
    uninitialized at return, the ownership face of the same missing fact:
    `conformance/probes/abort_after_mut_parameter_move.mojo`.
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
  - Probe: in a parameter slot, `f[[Float64(1.0), 2.0, 3.0]]()` over `def
    f[dtype: DType, //, c: Span[Scalar[dtype], _]]()` prints `3` at the
    pin, and run-time `h([Float32(1.0), 2.0])` over `def h[dtype: DType](s:
    Span[Scalar[dtype], _])` prints `2`; both are "cannot infer type
    parameter 'dtype'" here. The pin binds `dtype` from the converted
    parameter before the run-time argument:
    `polynomial_evaluate[[Float32(1.0)]](Float64(2))` is rejected at `x`.
  - Probe: `first(s)` over `def first(a: IndexList) -> Int` and a
    `StaticTuple[Int64, 2]` prints `4` at the pin. On landing, restore
    upstream's `_int_tuple_compare[apply_fn](self.data, rhs.data)` in
    `IndexList`'s five comparisons (`stdlib/std/utils/index.mojo`).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1774 An `@implicit` initializer over a type pack does not
  convert**

  `var t: IndexList[2] = (Int32(8), 9)` prints `(8, 9)` at the pin, through
  `IndexList`'s `@implicit __init__[*Ts: Movable & Intable](out self, elems:
  Tuple[*Ts])`. Mojito reports a type mismatch.
  - Cause: `Checker::conversion_constructor_parameter` and
    `solve_constructor_own_params` (`checker/declarations.rs`) solve a
    constructor's own binders from a plain pattern only, never a
    `Tuple[*Ts]` pack pattern from the source tuple's element types.
  - The explicit `IndexList[2]((Int32(8), 9))` checks, and a non-pack
    `@implicit __init__[T: Intable]` converts.
  - Probe: `take((1, 2, 3))` against `def take(a: IndexList[3])`.
  - Found while landing R1674 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1784 An `__init__` without `out self` that declares its result type
  is rejected**

  Problem: upstream's `@implicit def __init__(x: W[_]) -> W[x.n + 1]:
  return {}` is "'@implicit' initializers must accept a single positional
  argument value", where the pin prints `3` for `show(W[2]())` over `def
  show(w: W[3])`.
  - The form constructs another instance of the struct, whose type the
    signature computes from the argument.
  - `FloatLiteral`'s implicit initializer from `IntLiteral[_]` uses it
    (`#pop<int_to_float_literal>`, read as `ParamOp::IntToFloatLiteral`),
    so the bundled `float_literal.mojo` omits it.
  - Until it lands, `types::coerces` admits `IntLiteral[4]` into
    `FloatLiteral[4.0]` by rule, and the native backend stores the integer
    register unconverted. Probe: `var f: type_of(4.0) = 4; print(f)` prints
    `4.0` at the pin and on the VM, and fails Pliron's IR verification
    ("expected builtin.fp64 , got builtin.integer i64").
  - The MIR verifier's `types_compatible` (`verify/types.rs`) keeps one
    literal arm for the same rule: an `IntLiteral` register at a
    `FloatLiteral` slot. Landing this deletes it. Probe:
    `assets/ok/int_literal_parameter_value.mojo`'s `g(3)` binds
    `IntLiteral[3]` to `x: FloatLiteral` with no conversion.
  - Landing it also ports `IntLiteral`'s `__add__`, `__sub__`, and
    `__mul__` over `rhs: FloatLiteral` and its `__truediv__`, each typed
    `type_of(FloatLiteral(self) OP rhs)`. That deletes the `Div` arm of the
    literal rule in `Checker::infer_infix` for an `IntLiteral` pair.
  - Found while landing R1507 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2012 A lone positional collector's `@implicit` initializer
  converts nothing, so `raise 3` and `var e: Error = 5` are rejected**

  Upstream's `@implicit def __init__[*Ts: Writable](out self, *args: *Ts)`
  converts any `Writable` value, and a `raise` converts its operand to the
  error type through it. The pin prints `caught 3` and `5` for both
  probes.
  - `Checker::direct_conversion_constructor` (`checker.rs`) selects only an
    initializer whose first regular parameter is required. The declaration
    check (`check_method_inner`, `checker/declarations.rs`) already accepts
    the collector form.
  - `StmtKind::Raise` (`checker/statements.rs`) converts only a `String` or
    string literal operand. Upstream converts any operand the error type's
    `@implicit` initializers accept.
  - Probe: `raise 3` in a `raises` function is "raising operation produces
    'IntLiteral[3]', but this context propagates 'Error'".
  - Probe: `var e2: Error = 5; print(e2)` prints `5` at the pin.
  - Found while landing R1938 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1787 A bare parametric struct annotation on an initialized local
  does not infer its parameters from the initializer**

  Problem: `var x: W = W[3]()` over `struct W[v: Int]` reports "cannot
  infer type parameter 'v' of 'W' from the arguments", where the pin
  infers `W[3]` and prints `3` for `print(x.v)`.
  - The annotation resolves through `resolve_storage_annotation` under
    `StorageStrictness` as the struct with its parameters unbound, before
    the initializer's type is known.
  - Probe: `var x: IntLiteral = 3` reports the same "cannot infer type
    parameter 'value' of 'IntLiteral'", where the pin says "cannot
    implicitly convert 'IntLiteral[3]' value to 'IntLiteral[_]'"
    (`assets/type_error/int_literal_local_not_concrete.mojo`,
    `float_literal_local_not_concrete.mojo`). Once the rule infers, those
    two fixtures take the pin's text.
  - Probe: `var t: StringLiteral = s` over a `String` reports the same,
    where the pin says "cannot implicitly convert 'String' value to
    'StringLiteral[_]'" (`assets/type_error/nominal_string_default_rebind.mojo`),
    and `var b: StringLiteral = "q"` is "cannot implicitly convert
    'StringLiteral["q"]' value to 'StringLiteral[_]'" at the pin.
  - Found while landing R1780 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1857 A bare parametric struct as a result or `comptime`
  annotation reports a failed inference, where upstream rejects the
  annotation itself**

  Problem: `def g() -> W` over `struct W[v: Int]` reports "cannot infer
  type parameter 'v' of 'W' from the arguments", where the pin says
  "'W[_]' is not concrete, use '[]' to bind missing parameters".
  - Cause: outside a run-time parameter, a bare parametric struct
    annotation resolves as the struct with its parameters unbound, and the
    rejection comes later from inference, not from the annotation's
    position as upstream's does.
  - Probe: `comptime x: W = W[1]()` reports the same, where the pin says
    "cannot construct a value with parametric type: 'W[_]'".
  - Probe: `def g() -> StringLiteral` and `comptime x: StringLiteral =
    "a"` report "cannot infer type parameter 'value' of 'StringLiteral'",
    where the pin reports the two texts above for `'StringLiteral[_]'`.
  - Found while landing R1658 (2026-11-03).
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

- [ ] **R537 A list display of `Pointer` elements does not reach a constructor over the struct's origin parameter**

  Problem: `PtrList([Pointer(to=left)])`, whose `__init__` takes `var values: List[Pointer[Int, Self.o]]`, is rejected with "no constructor overload matches the supplied arguments"; the pin prints `6 6` after `pl.values[0][] += 2`.
  - The `Tuple` form, `PtrTuple((Pointer(to=left), Pointer(to=right)))`, runs.
  - The display's element is likely checked against the field type before the origin parameter is solved.
  - Found while planning R529 (2026-10-09).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2004 An explicit `f.__call__(x)` on a `def(...)`-bound value is
  rejected, and the checker does not spell `f(x)` as that call**

  `return f.__call__(x)` in `def apply[F: def(Int) -> Int](f: F, x: Int)`
  fails "type 'F' has no method '__call__'", where the pin reads the bound
  as a trait whose one requirement is `__call__` and prints `15`, `10`, `8`
  for a callable struct, a function, and a `{imm k}` closure.
  - The method lookup (`lookup_trait_methods`, `resolve_bound_method` in
    `checker/method_calls/resolution.rs` and `checker/traits.rs`) should
    synthesize the requirement's `MethodSig` from the `callable_bound`;
    its `method_lowered_name("__trait_dispatch", …)` must equal
    `callable_contract_target`.
  - Then `infer_call_method` drops its `callable_bound: None` restriction,
    so the checker spells `f(x)` as `f.__call__(x)`, as upstream's
    `CallNode` does, and MIR lowering's `bound_call_from_indirect`
    (`mir/bound_calls.rs`) goes.
  - That moves `infer_call`'s call-through residue and effect recording
    (`record_call_through`, `apply_call_through_effects`,
    `record_call_environment_effects`) onto the method path; the
    `assets/type_error` capture fixtures pin it.
  - Probe: the shape above.
  - Found while landing R1933 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2003 An indirect call still takes a struct callee, which the
  elaborator devirtualizes**

  `var g = add_k; print(g(5))` on a capture-list closure lowers to a
  `call.indirect` whose callee is the closure's storage struct, where
  upstream calls the storage's `__call__` method.
  - Once a closure storage declares `__call__` (R1943), the checker spells
    that call as a method call, and no struct callee reaches
    `call.indirect`.
  - Then delete the specializer `CallIndirect` arm's struct
    devirtualization (`native/mono/specializer.rs`) and
    `Self::callable_witness` if nothing else uses it,
    `verify_indirect_call`'s nominal branch (`mir/verify/calls.rs`), and
    Pliron's un-devirtualized nominal callable rejection
    (`lower/closures.rs`, `capability.rs`), and make a struct-typed
    `call.indirect` callee a verifier error.
  - Probe: `emit-mir` of the shape above holds no `call.indirect` on a
    struct-typed register, and it still prints `8` on both backends.
  - Found while landing R1933 (2026-11-07).
  - Depends on R1943.
  - Model: Opus, Not Planned.

- [ ] **R2005 A call through a generic `def[...]` bound is a `call.indirect`,
  not the bound's `__call__` requirement call**

  `f[2](40)` in `def invoke[F: def[n: Int](Int) -> Int](f: F)` lowers to a
  `call.indirect` with no dispatch symbol, where upstream calls the
  bound's generic `__call__` requirement, as R1933 made the monomorphic
  bound's call.
  - `MirInstr::MethodCall` has no `instantiated_contract` field, so the
    requirement call carries the bound's `param_decls` and solved
    `instantiated_args`, and the verifier recomputes the contract with
    `instantiate_generic_callable_contract`.
  - `bound_call_contract`, `indirect_from_bound_call`, and
    `register_ends::Callee::Contract` (`mir/bound_calls.rs`,
    `mir/register_ends.rs`) then admit a `GenericFunc` bound, and
    `verify_indirect_call` rejects that bound too.
  - Probe: `invoke(add)` over `def add[n: Int](value: Int) -> Int` prints
    `42` on both backends, and `emit-mir` of `invoke` holds a
    `call.method` of `__call__`.
  - Found while landing R1933 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2006 A struct field of a `def(...)`-bound type is rejected as
  non-`Deinitable`**

  `struct Holder[F: def(Int) -> Int]` with `var f: Self.F` fails "field
  'f' has non-'Deinitable' type 'F'", where the pin accepts it and
  `Holder(twice).run(4)` calling `self.f(x)` prints `8`.
  - The field rule in `checker/traits.rs` reads only the parameter's
    trait bounds; a `def(...)` bound's function-type trait implies
    `Deinitable` at the pin.
  - This also blocks reaching MIR lowering's field-invocation route of a
    bound call (`emit_indirect_invocation`) from source.
  - Probe: the shape above.
  - Found while landing R1933 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1515 A bracket followed by a call is fused into the call's
  parameter list**

  `x[args](...)` parses as one `Call { name, param_args }` or `Invoke {
  callee, param_args }` (`parse_bracket_suffix`, `parser/suffix.rs`),
  where upstream builds a `CallNode` over a `SubscriptNode` and the
  subscript is classified by its base before the call.
  - Probe: `print(t[1](4))` over `var t = (inc, inc)` prints `5` at the
    pin; Mojito reports "'t' has type Tuple and is not callable".
  - The checker re-dispatches a fused bracket as an element call only over
    a struct declaring `__getitem__` (`infer_call`,
    `checker/call_inference.rs`, and the `Invoke` arm of
    `checker/inference.rs`).
  - The lever: the parser keeps the subscript as the callee, and a callee
    that `Checker::bracket_callable` answers is the parameter application
    it is already spelled as; any other is the subscript's value, called.
  - Found while landing R1371 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1467 A call selecting `String`'s `Self`-typed `__eq__`/`__ne__`
  names an overload that was never lowered**

  The check records `String.__eq__$ov$String` for `a == b` (and
  `a.__eq__(b)`) over two `String`s, where the lowered declaration is
  `String.__eq__$ov$Self`; `__ne__` is the same.
  - The elaborator finds the declaration only through
    `symbol::resolve_callable_symbol`'s `Self`-shaped arity fallback,
    reached from `symbol::resolve_method_symbol`.
  - A struct of the program's own spells the selection right: `P == P` over
    `def __eq__(self, other: Self)` beside `def __eq__(self, other: Int)`
    records `P.__eq__$ov$Self`.
  - Probe: `print(a == b, a != b)` over two `String` values; done when
    `cargo run -- emit-mir` on it names no `String.__eq__$ov$String` or
    `String.__ne__$ov$String`.
  - Lever: spell the selection with the qualifier the selected signature
    carries (`MethodSig::overload`), as its declaration is lowered. The
    `Self`-shaped fallback then goes, and `resolve_method_symbol` becomes an
    exact lookup.
  - Found while landing R1281 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R55 A call through a `ref` to a callable value is rejected**

  Problem: `for f in fns: print(f(5))` and `ref g = fns[0]; print(g(1))` run at
  the pin over a function display, while Mojito reports "'f' has type ref
  def(Int) thin -> Int and is not callable".
  - Both thin and capturing elements are affected.
  - The indexed call `fns[0](5)` already works through element-call dispatch.
    A `ref`-typed callee has no such path.
  - Probe: `for f in fs: f()` in `def run[*fs: def() thin -> None]()` over
    `run[a, b]()` reports "'f' has type ref def() thin -> None and is not
    callable", where `fs[i]()` in a `comptime for` prints `a` and `b`
    (found while landing R1874, 2026-11-05).
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

- [ ] **R1838 A type-receiver call on a `SIMD` receiver is rejected**

  Problem: `Int.__add__(x, x)`, `Int8.__add__(x, x)`, and
  `Float64.__int__(2.5)` fail with "Undefined variable 'Scalar'", and
  `SIMD[DType.int8, 1].__add__(x, x)` with "expected Int8, found Int8",
  where the pin prints `2`, `2`, and `2`.
  - `infer_type_receiver_instance_call` (`method_calls/statics.rs`)
    refuses every `SIMD` receiver (`!actual.is_simd()`), and compares an
    explicit application's type arguments only, never a `SIMD`'s value
    slots.
  - A receiver spelled through a `comptime` alias of an application
    (`Int`, `Int8`, `Float64` over `Scalar[...]`) does not reach that
    function: the alias's head `Scalar` is looked up as a variable.
  - Upstream checks `T.method(x, ...)` as `x.method(...)` once `x`
    converts to `T`, whatever spelling names `T`.
  - Probe: the value read `var f = Int.__add__; print(f(1, 2))` fails
    with the same "Undefined variable 'Scalar'" where the pin prints `3`,
    while `List[Int].__len__` reads (found while landing R1795,
    2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2033 A `read` function does not convert to a function type
  taking `var`**

  Problem: `var a: def(var Int) thin -> Int = rd`, over `def rd(x: Int)
  -> Int`, is rejected with "expected def(Int) thin -> Int, found def(Int)
  thin -> Int", where the pin prints `2`.
  - Upstream converts a function value whose parameter is `read` to a
    type taking that parameter `var`: a callee that only reads accepts an
    owned argument. The reverse (`def(var x: Int)` to `def(Int)`) is
    rejected, as Mojito does.
  - The `Func` arm of `coerces` (`mojito-types/src/types.rs`) demands
    `conventions == to_conventions`.
  - Lowering must then pass the caller's owned argument to a `read`
    callee and destroy it after the call, on the VM's `CallIndirect` and
    in Pliron's `invoke` thunk.
  - Probe: `var b: def(var Int) thin -> G = G.__init__` over
    `@fieldwise_init struct G(Copyable): var x: Int` fails the same way,
    and `def(var x: Int) thin -> F[3] = F[3].__init__` over a parametric
    `@fieldwise_init struct F[n: Int]` does too, where the pin runs both
    (found while landing R1794, 2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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
  Some[Hasher])` is rejected as not conforming to `Hashable` ("does not
  match the signature required by trait"), while the pin accepts the
  conformance and runs it.
  - Trait conformance compares the witness's receiver convention with the
    requirement's exactly.
  - A bound dispatch's derivation relies on that equality today: it admits
    a witness only with the requirement's own receiver convention
    (`template_facts/bound_dispatch.rs:witness_binders`), and must judge the
    receiver again once a `mut` witness conforms.
  - Pinned by `conformance/probes/mut_self_hash_witness.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R92 A struct's callable-value parameter neither elaborates nor
  reads inside its methods**

  Problem: a struct instance whose parameter is a callable
  (`struct S[f: def() thin -> Int]`) fails at the elaborator, and a method
  reading the parameter fails too, where the pin runs both.
  - `var s = S[seven](1)` stops with "unsupported monomorphizing receiver
    for `S.__init__`: solution for `f` is not constant"; the pin prints
    `s.x`.
  - A method returning `self.x + Self.f()` stops with "Undefined variable
    'Self'"; the pin prints `8`. Bare `f()` is rejected at the pin
    ("unqualified access to struct parameter 'f'; use 'Self.f' instead"),
    and Mojito accepts it, then stops with "Undefined variable 'f'".
  - The same parameter on a module `def` runs: the call frame reifies it as
    a hidden local. A struct's deferred slot is never reified into its
    methods' frames.
  - Probe: a capturing closure naming an enclosing callable binder
    (`callable_frame` in `assets/ok/closure_storage_method_binders.mojo`,
    whose `body() {var x}` calls the frame's `f`) stays a generator nested
    in its enclosing declaration, taking its storage as a leading argument
    (`ClosureStorage::method`, `mir/nested.rs`), because its storage would
    carry `f`, where upstream's `liftClosureIntoMethod` makes every
    closure body its storage's method. Once a struct carries a callable
    parameter, drop that exception.
  - Such a closure has no value either: its storage has no `__call__`
    witness, so a first-class use of it lowers to `MirInstr::Unsupported`
    (`closure_storage_read`, `mir/lower_expr/calls.rs`).
  - Found while probing deferred slots (2026-09-26).
  - Depends on nothing.
  - Model: Fable, Not Planned.

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

- [ ] **R1111 An overloaded `__getitem__` over a parameter-spelled type does
  not lower**

  `struct Cell[n: Int]` with `__getitem__(self, a: Arr[Self.n])` beside
  `__getitem__(self, s: Slice)` checks, and then `c[a]` fails with
  "subscript refers to undeclared method `Cell.__getitem__$ov$Arr$Vn`",
  where the pin runs it.
  - The subscript's selected target spells the overload key from the
    parameter type `Arr[Self.n]` (`$ov$Arr$Vn`), while the method's
    lowered symbol is keyed another way, so the checked program names a
    method no declaration has.
  - One `__getitem__` over `Arr[Self.n]` lowers and runs
    (`assets/ok/dunder_unused_argument_instances.mojo`).
  - Found while landing R1033 (2026-10-17).
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
  - Probe: `def f[a: Int](x: Int)` beside `def f[T: AnyType](x: Int)` fails
    MIR verification ("value parameter 'a' has no runtime register"), where
    the pin prints `4 101` for `f[3](1), f[String](1)`. The key spells the
    explicit compile-time parameter names only for an overload with no
    run-time parameter (`SignatureKey::with_parameter_names`, R1550), and
    never their kinds.
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

- [ ] **R394 A requirement with its own value parameter cannot be called
  through a bound**

  Problem: `x.mk[4]()` in `def use[T: Maker](x: T)`, for `def mk[n:
  Int](self) -> SIMD[DType.int32, n * 2]: ...` in `trait Maker`, fails with
  "MIR function 'use' register r2 names parameter `n` of `Maker.mk` that no
  enclosing declaration binds", where the pin runs it.
  - The call on the conformer itself (`M().mk[2]()`) runs.
  - The result type through the bound keeps the requirement's binder
    instead of the call's argument.
  - The cause: a call through a bound records no instantiation of the
    requirement's own parameters (`record_method_call_instantiation` needs
    a struct receiver), so `instantiated_args` stays empty and the
    bracket argument keeps a run-time register.
  - Probe: `fp[n](b: PBox[n])` returning `pk(b)`, for
    `@always_inline("nodebug") def pk[T: Pick](a: T)` returning
    `a.pick[2]()`: the inliner folds the call to `PBox.pick`, but `fp`
    keeps `n` (two instances), where the pin and the direct call
    `b.pick[2]()` remove it.
  - Found while landing a function applied in a signature (2026-10-06).
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
  - Probe: `Self(7)` in a trait's default method reaches the same rejection
    through the `_Self` binder the default is checked over
    (`conformance/probes/trait_default_self_construction.mojo`).
  - Probe: `T.__init__[6]()` in `def make[T: K]() -> Int`, for `trait
    K(Deinitable)` requiring `def __init__[k: Int](out self)`, is rejected
    as unsupported ("a type parameter's construction"), where the pin
    prints `6` for `make[Q]()`. The explicit list rides
    `spelled_initializer_args` (`checker/initializer_list.rs`) and only a
    struct's `infer_construction_of` binds it.
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

- [ ] **R344 `rebind` cannot change a `Pointer`'s origin**

  Problem: `rebind[Pointer[String, origin_of(a)]](Pointer(to=b))` stops
  with "type mismatch for rebind: the input type does not match the result
  type", naming two origins, where the pin accepts it.
  - Upstream's `Tuple.reverse` and `Tuple.concat` retype their source
    pointers this way.
  - `checker/rebind.rs` compares the closed types whole, origins included.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R409 A violated `where` clause is quoted from its folded
  constraint, not as declared**

  Problem: `v.set[Conn](Conn(4))` on a `Variant[Conn]` whose `Conn` is not
  `Deinitable` notes "expected 'conforms_to(Ts.values, Deinitable)'". The
  pin quotes the declared clause, `Self.Ts.all_conforms_to[Deinitable]()`,
  as "expected 'TypeList.all_conforms_to[Deinitable]()'".
  - The cause: `violated_constraint_reason` (`checker/generics.rs`) prints
    the `Display` of the folded `GenericConstraint` (`ConformsPack` here),
    which keeps no trace of the clause's source expression.
  - Upstream's note prints the clause's own expression, rendering `Self.Ts`
    by its type, `TypeList`.
  - The lever: carry the declared clause on the constraint and quote it.
  - The pin's error line also differs: "no matching method in call to
    'set'", with a "candidate not viable: violated constraint" note.
  - `assets/type_error/variant_set_linear_alternative.mojo` expects only the
    note's prefix until this is fixed.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R420 A keyword call through a bound to a parameter a witness
  renames is rejected**

  Problem: `u.f(x=1, z=2)` through `U: T`, where a conformer declares
  `f(self, z: Int, x: Int)` for the requirement `f(self, x: Int, z: Int)`,
  fails with "binding 'x' of 'f' by name through a trait bound whose
  conformer renames it". The pin binds the keywords by the requirement's
  names and passes the values to the witness by position.
  - Probe: `conformance/probes/keyword_through_bound_renamed_witness.mojo`
    (the pin prints `312`).
  - The rejection is `Checker::record_bound_default_arguments`
    (`checker/bound_defaults.rs`). It also covers a requirement default the
    check would spell by a renamed name.
  - The cause: MIR keeps the call's keywords by name on its `call.method`
    to `__trait_dispatch.f$ov$Int$Int`, and `native::mono` binds them
    against the witness `S.f` it resolves the dispatch to. Without the
    rejection the probe prints `321`.
  - The lever: lower a call through a bound positionally, in the
    requirement's parameter order the checked call binding already records.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R548 A homogeneous variadic cannot be forwarded with `*xs`**

  Problem: `return g(*xs^)` in `def f(var *xs: Int)`, where `g` takes
  `var *xs: Int`, runs at the pin, while Mojito reports "call spread
  outside a specialized type pack".
  - The rejection is the `ExprKind::Spread` arm of inference
    (`checker/inference.rs`): only a spread of a type pack has a rule.
  - A homogeneous collector is already the bundled `VariadicList[T,
    is_owned]` in its callee's body, and a whole type-pack spread already
    passes the caller's pack itself. The lever is the same pass-through for
    the caller's `VariadicList`.
  - Probe: a trait default `def sum(self, var *xs: Int) -> Int` inherited
    by a conformer, called as `A(1).sum(1, 2, 3)`, prints `6` at the pin.
    Mojito rejects it in `unforwardable` (`checker/trait_defaults.rs`),
    since the conformer's forwarding wrapper spells that spread.
  - Found while landing R544 (2026-10-09).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R549 A trait method with a named `out` result is never satisfied**

  Problem: a requirement `def named(self, out r: String): ...` and a
  conformer spelling the same method print the result at the pin, while
  Mojito reports "struct 'S' method 'named' does not match the signature
  required by trait 'T'".
  - `check_trait` (`checker/traits.rs`) keeps the `out` parameter among the
    requirement's regular parameters, while a struct method's signature
    returns it.
  - Found while landing R544 (2026-10-09).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R550 A refining trait cannot restate an inherited requirement**

  Problem: `trait D(A)` declaring `def a(self) -> Int: ...` that `A`
  already requires is accepted at the pin, while Mojito reports "'a' is
  already declared in this scope".
  - Found while landing R544 (2026-10-09).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R570 A conformer method that does not satisfy a trait default
  still suppresses it**

  Problem: `trait T` defaulting `def f(self) -> Int` and `struct S(T)`
  spelling `def f(self) -> String` is accepted at the pin, which keeps the
  default's wrapper beside `S.f` and rejects only a call of either as
  ambiguous, while Mojito reports "struct 'S' method 'f' does not match
  the signature required by trait 'T'".
  - Mojito suppresses a default by its parameter shape
    (`trait_defaults::inherited_defaults`), upstream by satisfaction
    (`filterOverloadSetForValueType`).
  - Keeping both needs two methods of one parameter shape on one struct,
    whose symbols collide: `SignatureKey` carries no result type, and the
    `OverloadSets` prescan keys a name by arity.
  - Rationale: `docs/notes/trait-default-suppression.md`.
  - Found while landing R568 (2026-10-10).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R571 A conformer cannot inherit a trait default whose signature
  names an origin**

  Problem: a default `def pick(self, ref a: Int, ref b: Int) -> ref [a] Int`
  inherited by `struct S(T)` prints `4` at the pin for `S(1).pick(a, b)`,
  while Mojito reports that a wrapper over such a signature is not
  supported.
  - The check builds a conformer's wrapper from the default's resolved
    signature (`trait_defaults::default_wrappers`), and the method origin
    binders and origin clauses it would bind are erased from it.
  - `DefaultShape::unforwardable` rejects a receiver or parameter origin
    clause, a reference return, and an origin parameter.
  - Before R568 the cloned wrapper failed too, with "returned reference
    escapes storage outside its declared origin".
  - Found while landing R568 (2026-10-10).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R416 A pack forwarded into a plain struct's own-pack `__init__` is
  rejected**

  Problem: `Sink(*a)` in a pack-keyed `def`, against `def __init__[*Ts:
  Writable](out self, *a: *Ts)`, fails with "no constructor overload
  matches the supplied arguments", where the pin runs it.
  - A direct `Sink(1, "x")` runs, so the constructor's selection does not
    bind a forwarded pack as a method call's does.
  - A constructor spread is a call, not a method call, so serving the
    spreading `def` once it checks is R316's.
  - Found while landing R256 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R593 A trait member applied to a conformer is rejected with the
  wrong words**

  Problem: `T.d[S](S(3))` stops with "Undefined variable 'T'", where the
  pin says "Direct access of trait members is not supported.".
  - `T.d(self)` already reports the pin's words
    (`assets/type_error/trait_member_direct_access.mojo`); the bracketed
    application takes another path.
  - Found while landing R551 (2026-10-10).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1152 A typed raise does not convert to the `Error` its context
  propagates**

  Problem: `print(g())` inside `def f() raises:` for `def g() raises
  StopIteration -> Int` is rejected ("raising operation produces
  'StopIteration', but this context propagates 'Error'"), where the pin
  runs it and an `except e:` around `f()` prints `caught StopIteration`.
  - `Checker::require_error` (`checker.rs`) accepts only an error type
    equal to the context's.
  - Probe: `ParameterList.__iter__().__next__()` read in a plain `raises`
    function.
  - Found while landing R807 (2026-10-19).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1250 A `match`'s pattern tests carry the statement's location**

  Problem: every test, path read, and binding a `match` or `case`
  condition synthesizes sits at the statement's source location, where
  upstream locates each command at its pattern (`cmd->expr`) and a
  cluster's elif condition at its group's first pattern.
  - `Synth::for_statement` (`checker/desugar.rs`) gives every node it
    builds the statement's span.
  - Only native line tables show it: a pattern test steps on the `match`
    line, and neither compiler's run-time error names a location.
  - Lever: a `Synth` per pattern for the commands it compiles, keeping
    syntax ids unique per statement.
  - Found while landing R1140 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1322 Slicing a list bound from a display is rejected**

  `var xs = [1, 2, 3, 4]` then `xs[1:3]` stops with "invalid call to
  '__getitem__': no overload matches the supplied arguments", where the pin
  slices it (`len(xs[1:3])` prints `2`).
  - A `List[Int]()` filled by `append` slices, so the display's inferred
    element type is the likely difference.
  - Found in the 2026-10-24 overnight-gate triage, beside
    `benchmarks/compile/collections_many.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1412 A struct field of a generic library type is not implicitly
  copyable under its struct's conditional `ImplicitlyCopyable`**

  `struct R[T, E](Copyable, ImplicitlyCopyable where conforms_to(T,
  ImplicitlyCopyable) and conforms_to(E, ImplicitlyCopyable))` with `var v:
  Variant[Self.T, Self.E]` is rejected ("field 'v' has type 'Variant[T, E]',
  which is not ImplicitlyCopyable"), where the pin runs it.
  - `struct_implicitly_copyable_conformance_ok` (`checker/traits.rs`)
    assumes the condition for a field of a parameter's type and for private
    storage over parameters, not for a library struct whose own conditional
    conformance the condition implies.
  - Found while landing `enum` (2026-10-25).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1413 Enum declaration errors a payload's type causes use Mojito's
  words**

  The pin's "case 'list' has non-'ImplicitlyCopyable' payload type
  'List[Int]'", "enum case payloads do not support trait types; 'AnyType' is
  a trait, ...", and "deriving `__eq__` is not yet supported for enums;
  write `__eq__`" are reported as a field conformance failure, a
  conformance-oracle failure, and an undefined operator.
  - The verdicts agree
    (`assets/type_error/enum_payload_not_implicitly_copyable.mojo`).
  - Found while landing `enum` (2026-10-25).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1750 An unknown name in an overloaded call's argument reports no
  matching overload**

  `h(Foo(1))` over two `h` overloads, with `Foo` undeclared, reports
  "invalid call to 'h': no overload matches the supplied arguments", where
  the pin reports "use of unknown declaration 'Foo'".
  - Cause: overload resolution judges each candidate's arguments and
    folds an argument's own error into the candidate's rejection, so an
    error no candidate can avoid reaches the user as a mismatch.
  - Probe: `c.append(Foo(1))` over a `List` reports the same mismatch.
  - Probe: `c.__getitem__(ContiguousSlice(None, 2, None))` without
    importing `ContiguousSlice` reports it, in a `comptime` binding and
    at run time alike.
  - Found while landing R1742 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1776 A call's arguments are re-inferred for every overload
  candidate, so nested constructions cost candidates to the power of their
  depth**

  `c += Int(binary_fn(Scalar[DType.int64](a[0]), Scalar[DType.int64](b[0])))`
  in a function over `a: StaticTuple[Int64, _]` and a callable binder takes
  3 s in a debug `mojito check`; each piece alone takes milliseconds.
  - Cause: `infer_construction_of` (`checker/declarations.rs`) scores each
    `__init__` candidate by re-inferring every argument
    (`instantiate_method_generics`, `score_method_call`,
    `solving_argument_ty`), and the selected candidate re-runs, so an
    argument that is itself a construction is checked once per outer
    candidate: about 30 `SIMD` initializers per level.
  - Lever: infer each argument's bare type once per call and replay its
    journaled facts for later candidates, rather than re-checking the
    subtree.
  - Probe: upstream's `_int_tuple_binary_apply`
    (`stdlib/std/utils/index.mojo`) dominates that module's share of the
    package build.
  - Probe: `Int(Scalar[DType.int64](a.get[0]()))` alone takes 0.3 s.
  - R137's nested `wrap(wrap(…))` may share this cause.
  - Found while landing R1674 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1926 A default value cannot bind an origin parameter of its
  parameter's type**

  `def f[O: ImmOrigin](s: StringSpan[O] = StaticString("d"))` is rejected
  at its declaration with "type mismatch for default value of 's':
  expected StringSpan, found StringSpan", where the pin binds `O` to the
  default's static origin when the call leaves `s` out.
  - The default is checked against the declared type with `O` still a
    parameter, which no concrete origin coerces to.
  - Probe: the same on a method, and `f()` and `f(StaticString("x"))`
    printing `d` and `x`.
  - Probe: the bundled `VariadicPack._write_to` takes `start`, `end`, and
    `sep` as `StaticString` where upstream takes `StringSlice[O1]` (and
    `O2`, `O3`) defaulted to `StaticString("")`.
  - Found while landing R1302 (2026-11-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Catch Up To Current Mojo: Compile-Time Parameters, Packs, And Reflection

Track: `comptime`.

Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

- [ ] **R2019 A Bool-bodied `comptime` alias accepts `T.values` of a
  parameter that is not a pack**

  `comptime AllCopy[T: AnyType] = conforms_to(T.values, Copyable)` is
  accepted, where the pin rejects it at the declaration with "expected a
  type or param_list operand", since `T` is not a variadic pack.
  - `checker_test::bool_bodied_comptime_aliases_are_predicates` pins the
    rejection ("a pack projection in a Bool-bodied comptime alias",
    `crates/mojito-checker/src/checker/constraints.rs`) and fails today:
    the body no longer lowers to a `ConformsPack`, so the check never
    sees a pack projection.
  - Fix: reject `.values` of a non-pack parameter where the body is
    resolved, with the pin's diagnostic, then let the test assert that.
  - `tests/focused/list.txt` leaves the test out until this lands.
  - Found while landing R1999 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2022 A local `comptime` value does not shadow a module type of
  its name in a type annotation**

  Over a module `struct L[n: Int]`, `comptime L = TypeList.of[Trait=AnyType,
  Int, Bool]()` in `main` and `var b: L[1] = True` fail with "expected
  L[1], found Bool", where the pin reads the local and prints `True`.
  - A name resolves through the innermost scope at the pin, so the local
    binding hides the module struct in every position.
  - `Checker::ty_from_anno`'s `SourceType::Named` arm
    (`checker/type_resolution.rs`) asks `applied_type_at_depth` first and
    reads the name as a compile-time value (`value_headed_type`) only
    when no type has that name, so a module type wins over a local value.
  - Found while landing R1540 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2007 A struct's `comptime` member that constructs the struct
  through an initializer typed by a struct alias does not evaluate**

  `comptime int64 = DT(mlir_value=…)` over `def __init__(out self, *,
  mlir_value: Self._mlir_type)` fails "not a compile-time Int constant:
  not an associated comptime expression", where the pin builds the member.
  - Cause: `classify_applicable_signatures` (`comptime_validation.rs`)
    resolves an applicable callable's parameter types before the struct's
    own `comptime` members register, so `Self._mlir_type` does not
    resolve, and `check_struct_associated` evaluates those members before
    any later run retries the classification. `applied_arguments` then
    finds no signature and the construction is no application.
  - The same initializer with its parameter spelled `!kgen.dtype`
    directly (a scalar slot), or typed `index`, evaluates.
  - Probe: upstream's `struct DType` copied verbatim as a user `struct DT`
    fails so at its first member, `comptime bool`.
  - Probe: the bundled `struct DType`'s keyword `__init__` spells
    `!kgen.dtype` where upstream writes `Self._mlir_type`
    (`stdlib/std/builtin/dtype.mojo`'s header); restore upstream's
    spelling once this lands.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2002 A lifted `comptime for` over a value pack passes its
  `ParameterList` to `__iter__` as a register, which the VM refuses as a
  reference receiver, with the pre-elaboration pipeline off**

  With `MOJITO_PRE_ELABORATION=0`, `def total[*values: Int]() -> Int`
  summing `comptime for value in values` and called as `total[1, 2, 3]()`
  fails with "VM CTFE failed for '$comptime$total$c0…': vm: reference
  receiver for method 'ParameterList….__iter__' must be a place", where
  the pin and the default pipeline print `6`.
  - The lifted thunk calls `__iter__(ref self)` on the `ParameterList`
    value its construction leaves in a register; upstream's lowering
    materializes an rvalue receiver of a `ref self` method into a
    temporary before the call.
  - With the pipeline on, `inline-parametric` and `mem-2-reg` remove the
    call, so only the pipeline-off path reaches the VM's check.
  - Probe: the program above, and `assets/ok/bracket_function_application.mojo`,
    both with `MOJITO_PRE_ELABORATION=0`.
  - Found while landing R2001 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1942 A lifted compile-time expression calling a local
  `comptime` closure fails in CTFE, where the pin runs it**

  A local `comptime` binding of a lambda is evaluated again in the
  prologue of each lifted function that reads it, so the lifted function
  calls the closure through a `def(...)` contract's indirect call, which
  VM CTFE does not run; upstream's binding is an alias of the closure
  itself, which the interpreter calls directly.
  - Probe: `comptime f = lambda (x: Int) {} -> Int: x * 2`, then
    `comptime w = f(21)` and `print(w)` in `main` fails with "unsupported
    VM CTFE failed for '$comptime$main$c0': ... callee
    '__trait_dispatch.__call__$ov$Int'"; the run-time `print(f(4))`
    alone prints `8`.
  - Lever: make the binding the closure's alias (the check's
    `denotes`), as a binding of an application already is, so the lifted
    function names the owner's lowered lambda instead of re-binding it.
  - Found while landing R1940 (2026-11-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2042 A `case` pattern's value builds a checked node of its own
  beside the one inside the case's planned test**

  The checked arena builds a node for each expression a `case` pattern
  embeds (`CheckedPattern`), and again for the clone of that expression
  the pattern compiler put in the case's planned test (`subject ==
  value`) or `comptime match` condition, so one source occurrence builds
  two nodes.
  - Upstream's pattern is lowered into its tests: the value is one
    operand of one comparison, never a node of its own.
  - Lever: let the checked pattern name the test's operand, or carry no
    expression nodes at all, and move the readers of
    `CheckedPattern::expressions` (HIR, the comptime rewrite) to the
    commands.
  - Probe: `checked_boundary_keys_template_source_provenance_by_occurrence`
    in `tests/checker_test.rs` with `MOJITO_STD_PACKAGE=0` and the
    whole-program check (`checked.expressions()` instead of
    `own_expressions`) reports 99 duplicates, every one a pattern value:
    `case "bool":` and `case .bool:` in `DType`, `comptime match width:
    case 8:`, and `comptime match dtype: case .float16:` in
    `utils/numerics.mojo`, `math/math.mojo`, and `simd.mojo`.
  - Restore that test's whole-program check with the fix.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1650 The elaborator binds a callable binder to its argument's
  type with the environment erased, so `conforms_to` over a thin function
  answers as for a contract**

  `comptime if conforms_to(F, TrivialRegisterPassable)` in `def probe[F:
  Copyable & def(Int) -> Int](f: F)` takes the false arm for `probe(inc)`
  over a module `def inc`, where the pin takes the true arm; a closure
  with trivial captures takes the true arm on both.
  - Cause: `bind_type` (`native/mono/unify.rs`) stores the binder's
    solution through `canonicalize_callable`, which rewrites every
    `thin` and `capturing[...]` environment to the unqualified contract's,
    so the specializer's `type_conforms` asks about `def(Int) -> Int`,
    not `def(Int) thin -> Int`.
  - Upstream's thin and capturing function types are distinct types, so
    they are distinct instances; keep the environment in the binder's
    solution and in instance identity, and erase only what the runtime
    ABI erases (capture origins).
  - Probe: the `RegisterPassable` arm of the same body is skipped too.
  - The checker already answers both conformances for the thin argument
    (R1584), so the gap is the elaborator's alone.
  - Found while landing R1584 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1115 `where <cond> else "<message>"` is the preferred constraint
  message**

  Problem: `def foo[sc: Int]() where sc > 1 else "scaling factor must be
  greater than 1":` fails to parse ("Unexpected token Else: Expected ':'
  before the function body"), where the pin reads it as the preferred
  spelling of `where (sc > 1, "...")`, which it will deprecate over time.
  - A struct's conditional conformance takes the same tail:
    `Marker where conforms_to(T, Marker) else "Box[T] is a Marker only
    when T is"`.
  - Lever: `where_clause` reads an optional `else STRING` after its
    expression into the (condition, message) pair the tuple form fills.
  - `docs/grammar.md` first, then parser tests and fixtures beside the
    tuple-form ones.
  - Probe: the bundled `MaybeUninit` spells its three conditional
    conformances in the tuple form, where upstream's use `where ... else`.
  - Probe: the bundled `VariadicPack` declares `Copyable where not
    is_owned` without upstream's `else "Cannot copy an owned variadic
    pack."`.
  - Found at the 2026-09-21 re-pin and left unfiled; refiled at the
    2026-10-17 re-pin.
  - At `1.3.0.dev2026110105` `@explicit_destroy("<message>")` warns as
    deprecated. Upstream's `Tuple`, `Array`, `List`, `Dict`, and `Set` give
    the message on the conformance instead (`Deinitable where ... else
    "..."`), and it is reported when a value is abandoned for parameters
    that fail the condition. Respell the bundled library's
    `@explicit_destroy` uses the same way once this lands.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2026 A user pack struct's explicit copy initializer rejects
  `P[Int, String](copy=p)`**

  Over `struct P[*Ts: Movable]` declaring `__init__(out self, *, copy:
  Self)` that runs `self = Self(copy.n + 1)`, `P[Int, String](copy=p)` is
  rejected with "type mismatch for argument 'copy' to 'P.__init__':
  expected P[*Ts], found P[(Int, String)]", where the pin prints `4`.
  - The `copy` parameter's `Self` stays at the template's binders where
    the call binds the bracket's list; a struct that binds its pack
    element-wise (`Tuple`) is not affected.
  - Probe: the struct above with `var n: Int`, `def __init__(out self, n:
    Int)`, then `var p = P[Int, String](4)` and the copy.
  - Found while landing R341 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1120 `Tuple[]` is the empty tuple's type**

  Problem: `def f() -> Tuple[]:` fails to parse ("Unexpected token
  RBracket: Expected an expression"), where the pin spells the empty
  tuple's type `Tuple[]` and nothing else: `()` is a tuple value ("expected
  a type, found a tuple value; use 'Tuple[...]' to write a tuple type"),
  which Mojito already rejects as a type.
  - Lever: `parse_type` admits an empty argument list on `Tuple`, and the
    checker resolves it to the zero-element tuple.
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1646 A construction spelled through a closed type alias is no
  compile-time application**

  `comptime u = WW(5)` then `f[u]()`, over `comptime WW = W` and `def
  f[w: W]()`, fails with "not a compile-time Int constant: u", where the
  pin prints `5`; `comptime u = W(5)` runs.
  - `Checker::selected_application` (`checker/comptime_validation.rs`)
    looks the callee's name up as a function, so an alias name finds
    nothing. A run-time call through the alias already constructs
    (`infer_type_alias_read`).
  - Lever: resolve a callee naming a type alias to its struct template
    and spelled instance (`ty_from_anno`), as the `on_instance` arm does
    for `G[2](n)`.
  - Probe: `comptime t = StaticString("lit")` then `name[t]()`, and
    `name[StaticString("lit")]()`, over `def name[s: StaticString]() ->
    String`, which print `lit` at the pin. The second fails with "not an
    associated comptime expression".
  - Probe: upstream's `StringLiteral.format` body passes
    `_FormatUtils.format_to_comptime[StaticString(Self())]`; the bundled
    one binds the template to a local `comptime template` first (found
    while landing R1655).
  - Probe: `f[Int32(n)]()` and `f[Int32(from_int=n)]()` in `def g[n:
    Int]()`, over `def f[v: Int32]()`, print `7` for `g[7]()` at the pin.
    Mojito fails with "a SIMD[DType.int32, 1] lane needs a compile-time
    scalar" and "not an associated comptime expression". The first is
    `Checker::staged_simd_construction`'s vector-alias arm
    (`checker/constraints.rs`), which goes once an alias call applies its
    `SIMD` initializer.
  - Probe: `g[Float32(from_bits=UInt32(1065353216))]()` over `def g[v:
    Float32]()` prints `1.0` at the pin and fails with "not an associated
    comptime expression", while `g[SIMD[DType.float32, 1](from_bits=…)]()`
    runs; `g[Float32(UInt32(7))]()` fails with "a SIMD[DType.float32, 1]
    lane needs a compile-time scalar" (found while landing R1593).
  - Probe: `comptime u = Int32(5)` and `U256(0)` fold through
    `Checker::literal_initializer_body` (`checker/constraints.rs`), the
    attribute `SIMD.__init__(IntLiteral)`'s body folds to, rather than
    applying the declared initializer through the alias (found while
    landing R1781).
  - `comptime_requests.rs`'s `CalleeOracle::callee` answers `Other` for a
    call through any struct alias (`P(2)` over `comptime P = Pt`,
    `Int32(5)`), so `applies_callable` never lifts it; it must resolve a
    type alias to its struct too, as `SIMD[...](x)` and `Pt(1)` already
    lift (found while landing R1791).
  - Found while landing R1641 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1693 A compile-time call of a nested `def` applies a lifted
  thunk, where the pin applies the nested `def`'s generator**

  `first[mk()]()` and `comptime t = mk()` over a nested `def` that
  captures nothing denote `$comptime$outer$c0[length]`, a function the
  check lifts whose body calls `outer$mk`, where the pin applies
  `outer.mk`'s generator to the enclosing parameters.
  - Cause: a parameter expression's application names its function by
    lowered symbol, and a nested `def`'s symbol (`CheckedDef::symbol`) is
    named only by the check's last step (`collect_closure_uses`), after
    the application is selected.
  - `Checker::selected_application` skips a local callee
    (`binding_scope == 0`), and `applicable_functions` lists module
    declarations only.
  - Lever: list a captureless nested `def` as an `ApplicableFunction`
    under the symbol MIR gives it, its enclosing binders leading in
    `binder_scope` order, and bind them in
    `Specializer::demand_application` as `nested_bindings` does.
  - Trap: `split_method_symbol` of `W.go$mk` names `W`, so
    `demand_application`'s owner arm would read the first argument as a
    `W` instance.
  - No output differs. Probe: the MIR of
    `assets/ok/nested_def_comptime_struct_result.mojo` applies
    `$comptime$outer$c…` functions where it should apply `outer$mk`.
  - Found while landing R1686 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1688 A run-time call binding a struct-typed value pack from
  literals fails elaboration**

  `var u = StaticTuple[SIMDLength, 4].__init__[7, 2, 5, 0]()` fails
  elaboration with "cannot resolve parameter `*values`", where the pin
  prints `7` for `Int(u[0])`.
  - The `Int` element form runs, and so does the `SIMDLength` pack
    forwarded from a binder (`__init__[*mask]` in `def tup[*mask:
    SIMDLength]`).
  - The literals convert to `SIMDLength` through its `@implicit`
    initializer, and the call's pack keeps those applications, which the
    instance does not fold to values.
  - Probe: `comptime u = StaticTuple[SIMDLength, 4].__init__[7, 2, 5,
    0]()` fails the same way.
  - Probe: `first[StaticTuple[SIMDLength, 2].__init__[4, 1]()]()` over
    `def first[t: StaticTuple[SIMDLength, 2]]()` stops with "not an
    associated comptime expression", where the pin prints `4`. As an
    application's argument the pack must be one parameter expression, and
    `ParamContext::constant` refuses a tuple holding the `SIMDLength(4)`
    applications. The `Int` form and the forwarded `[*mask]` apply.
  - Found while planning R1671 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1513 Compile-time consumers read a bracket application only as
  `TypeApply`**

  About two dozen checker and comptime sites match `ExprKind::TypeApply`
  alone, where upstream's sema reads one `SubscriptNode` whatever its
  operands, so the parser cannot yet emit `Index`/`MultiIndex` for a
  capitalized, keyword, or type-operand bracket (R1514).
  - The lever: each site reads `Expr::bracket_application`
    (`crates/mojito-ast/src/ast.rs`) and keeps its own check of what the
    name denotes. It is additive: the parser still emits `TypeApply`.
  - Sites that match only `TypeApply`: `infer_reference_value` and
    `infer_variant_storage_method` (`checker/inference.rs`),
    `check_ct_bool` and `comptime_type_operand`
    (`checker/comptime_validation.rs`), `lower_parameterized_member`
    (`checker/constraints.rs`), `places.rs` `transfer_root`,
    `statements.rs`'s comptime type-alias route, `applied_member_root`
    (`checker/indexing.rs`), `origins/actuals.rs` and `origins/sig.rs`,
    the type-parameter and callable defaults (`checker/declarations.rs`),
    `fold_simd_alias_bounds` (`comptime/synth.rs`), `retype_head`
    (`comptime/rewrite.rs`), `simd_width.rs`, and the pattern heads in
    `checker/patterns.rs`.
  - Sites that read `Index` but not `MultiIndex`, so `Name[a, b]` and
    `Name[k=v]` escape them: `type_spelling` (`ast.rs`),
    `symbol::spelled_type_argument`, `comptime_validation.rs`,
    `constraints.rs`, and `comptime.rs`.
  - Sites that build `ParamArg::Value(index)` by hand, so neither unwrap a
    `TypeValue` nor split a tuple: `comptime_validation.rs`,
    `type_resolution.rs`, `indexing.rs`, `ast.rs` `type_spelling`,
    `comptime.rs`, and `variant_projection_type_argument`
    (`inference.rs`). They switch to `subscript_param_args`.
  - `Checker::bracket_callable` (`checker/generics.rs`) treats a local
    holding a function value as an overload set; upstream binds only a
    declared `def`. The lever: test `lookup_callable_origins`.
  - Found while landing R1371 (2026-10-27).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1514 A capitalized, keyword, or type-operand bracket is parsed as a
  parameter application**

  `parse_bracket_suffix` (`parser/suffix.rs`) still builds `TypeApply` for
  a bracket with several operands or a keyword operand over a capitalized
  base, or with an operand that parses as a type, where upstream builds one
  `SubscriptNode` and sema classifies it by its base.
  - The lever: the tail of `parse_bracket_suffix` becomes one conversion.
    Positional operands are `SubscriptArg::Index`, keyword operands
    `SubscriptArg::Keyword`, and a type operand `TypeValue(ty)`; the
    result is `Index` for one positional operand, else `MultiIndex`.
  - `Type::None` becomes `ExprKind::None`, which `subscript_param_arg` reads
    back as `ParamArg::Type(Type::None)`. The `None` and `Self`-chain cases,
    `parameter_application`, `call_name`, `expression_name_starts_lowercase`,
    and `self_param_rooted_expression` go.
  - Also: the value category of a type-denoting subscript
    (`crates/mojito-checked/src/checked.rs`), `rewrite_spelled_param_args`'
    re-keying for the bracket shapes (`comptime/rewrite.rs`), the parser
    tests' `TypeApply` assertions, and `docs/grammar.md`'s `subscript_arg`
    "value bases only" note.
  - Probe: `comptime U256 = SIMD[DType.uint64, 4]`
    (`stdlib/std/hash/_ahash.mojo`) must stay a type alias; an
    unconverted parser flip stops the bundled std packaging there.
  - Found while landing R1371 (2026-10-27).
  - Depends on R1513.
  - Model: Fable, Not Planned.

- [ ] **R1511 A scalar type name in a bracket is a type by its spelling**

  `peek_starts_type` (`parser/types.rs`) sends `Int`, `UInt`, `Bool`,
  `StringLiteral`, `Float64`, and the sized aliases (`UInt8`) to
  `parse_type`, where upstream parses every bracket operand as one
  expression and sema classifies it.
  - Dropping the sized aliases alone breaks `UnsafePointer[UInt8]`
    declaration identity (14 corpus fixtures,
    `String.__init__$ov$UnsafePointer$UInt8$kw$unsafe_from_utf8_ptr`).
  - Dropping all five names also changes five diagnostics
    (`convert[Int]` into a `DType` slot, `objs[Int](3)`,
    `UnsafePointer[Int].alloc`, `g[Int]()` into a trait binder, `k[Int]()`
    into `!kgen.type`) and stops `var element: M.Of[Int]()[0]` parsing.
  - The lever: `symbol::ast_raw` spells a bare type operand as the type it
    names, excluding `_`/`...`, binders, `Int` constants, and `DType`
    members (measured 0 corpus diffs while planning R1370); naming every
    declaration from its resolved signature (R1770) is the full fix.
  - Probe: `materialize[Int8.MAX]()` and `materialize[Int32.dtype]()`
    print `127` and `int32` at the pin. Mojito parses the operand as a
    type path (`ParamArg::Type(Assoc)`), so
    `Checker::infer_template_materialize` reports
    "materialize[...]() takes one compile-time value"; through
    `comptime x = Int8.MAX` it runs.
  - Found while landing R1370 (2026-10-27).
  - Depends on R1514, for the single-operand `Index` forms.
  - Model: Opus, Not Planned.

- [ ] **R1512 `Self`, `Self.T`, and `None` in a bracket are types by their
  spelling**

  `peek_starts_type` (`parser/types.rs`) parses `Self`, a `Self` member,
  and `None` in a bracket as a type, where upstream parses each as an
  expression.
  - As expressions the bundled std does not package ("unknown type
    'Self'"): about 20 `ParamArg::Type(SourceType::SelfParam(..))` arms in
    the checker and `mojito-comptime/src/comptime/rewrite.rs` read the
    type spelling.
  - The end state is `ParamArg::Type` only for `def(...)`, `ref`, and
    `*Ts`, which only the type grammar spells.
  - Found while landing R1370 (2026-10-27).
  - Depends on R1511.
  - Model: Fable, Not Planned.

- [ ] **R1244 A compile-time arm does not assume that the earlier
  conditions failed**

  Problem: `comptime if not conforms_to(T, Writable): ... else: print(x)`
  is rejected ("an element of 'values' with type 'T' does not conform to
  trait 'Writable'"), where the pin prints `1`, and so is a `comptime
  match` case after `case U if not conforms_to(U, Writable)`.
  - Upstream's `buildBranchAssumption` (`emitComptimeCases`) gives each
    arm its own condition and the negation of every earlier arm's, so an
    `else` arm knows that every condition failed.
  - Mojito's arms assume their own condition alone:
    `check_conditional`'s compile-time branch (`checker/statements.rs`)
    and `check_comptime_case` (`checker/patterns.rs`) call
    `assume_condition` over that condition only.
  - Lever: assume each earlier condition's negation too, through
    `conformance_arm_assumptions`, which must then read a `conforms_to`
    atom under a double negation.
  - Found while landing R1139 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R972 A generic caller omitting a parameter whose default reads
  an earlier one is rejected**

  `fwd[x]` whose body is `return default_only[x]()`, where
  `default_only[a: Int, b: Int = a]`, fails with "not a compile-time Int
  constant: default for parameter 'b'", where the pin prints `1`.
  - The same call from `main` with a literal argument works, so the
    default is evaluated before `a` is bound to the caller's symbolic `x`.
  - The default should stay a parameter expression over the caller's
    binders, as an explicit `default_only[x, x]()` does.
  - Probe: a struct-typed default over an earlier binder, `h[n: Int, w: W
    = W(n)]` or the converted `w: W = n` (`W`'s `@implicit __init__(Int)`
    storing `value * 2`), fails the same way in `default_argument` for
    `h[4]()`. The pin prints `8`.
  - Add the probe as an `assets/ok` fixture once it runs.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R527 An explicitly specialized generic `def` converts to a plain function type**

  Problem: `apply(ident[Int], 1)` against `cb: def(Int) -> Int` checks, then fails at run time with "'ident' is not callable"; the pin rejects the conversion.
  - The unspecialized `ident` is already rejected with the pin's message.
  - Reject the specialized form at the call argument the same way.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R531 The non-`Deinitable` field rule covers only a `Tuple` over a pack**

  Problem: a struct field of type `Tuple[*Self.Ts]` now needs a pack bound proving `Deinitable`, as at the pin, but a field of any other struct whose `Deinitable` conformance depends on its arguments is not checked.
  - The rule lives in `checker/traits.rs`.
  - Generalize it to the field type's conditional `Deinitable` conformance.
  - Probe: `struct Bag[T: AnyType]` holding `var items: List[Self.T]` is
    accepted, where the pin reports "field 'items' has non-'Deinitable'
    type 'List[T]'".
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R770 A struct parameter expression as a method's bracket argument
  lowers as a run-time read of `self`**

  Problem: in `struct Holder[w: Int]`, `s.scale[Self.w + 1](5)` for a
  method `def scale[n: Int](self, value: Int)` fails MIR verification
  ("compile-time value parameter 'n' has register type Holder, declared
  Int"); the pin runs it.
  - A bare `Self.w` argument runs, and so does `scale_free[Self.w + 1](5)`
    for a free `def`.
  - So the method-call path lowers the compound argument's `Self` as the
    receiver register instead of the struct's parameter value.
  - Found while landing R590 (2026-10-13); HEAD before R590 fails the same
    way.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R528 A default value is evaluated at each call; the pin evaluates it once at compile time**

  Problem: `def fd2(x: Int = side())` returning `x`, where `side` prints, prints `side` at each `fd2()` call, where the pin evaluates the default once at compile time: `mojo run` prints `side` once before `main` runs, and a `mojo build` binary never prints it.
  - `mir::fill_omitted_arguments` writes the default's evaluation into each call, a `$default$` call or an `@implicit` conversion's constructor call.
  - Probe: `conv(a: Int, l: Loud = 3)` with an unused `l`, where `Loud`'s `@implicit __init__` and `__deinit__` print, called as `conv(1)`: Mojito prints `init 3` and `del 3`, the pin's binary only `del 3`. `erase_unpassed_defaults` erases only a `$default$` call, since the conversion's constructor call is spelled as a written one.
  - Probe: `noisy(a: Int, n: Noisy = Noisy())` with an unused `n`, called as `noisy(4)`: Mojito runs neither `Noisy()` nor its destructor, the pin destroys the value it evaluated at compile time, printing `del` at each call.
  - Evaluate a default once at compile time, as the pin does, and materialize the value at each call.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R522 Aliasing inside a frozen value is not preserved**

  Two pointer fields into one compile-time allocation freeze as independent copies.
  - `CtValue::Pointer` retains contents and offsets without allocation identity.
  - Preserve an allocation graph when supporting materializable shared storage;
    cyclic memory currently rejects explicitly.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R480 A `comptime if` over an element of a vector-valued binder
  takes the wrong arm**

  Problem: `a5[v: SIMD[DType.int32, 2]]() -> Int32` holding
  `comptime if v[0] > 0: return 7` then `return 9` prints `9` for
  `a5[SIMD[DType.int32, 2](1, 5)]()`, where the pin prints `7`.
  - A plain read of the element (`return v[1]`) is right.
  - `--erased` reports "the erased oracle cannot decide the comptime if
    condition", so the condition is not folded from the binder's value.
  - Found while landing R477 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R508 A body's requested `List` binding cannot be read at compile
  time**

  Problem: `comptime L = mk()` over `def mk() -> List[Int]`, then
  `print(comptime(len(L)))`, prints `3` at the pin and stops in Mojito with
  "vm backend does not support methods on None yet".
  - The binding is a request the elaborator below MIR serves, and the
    compile-time read of it is checked as an ordinary `len` of a value no
    one demanded.
  - A `comptime for` over the same binding works, since its header is the
    demand.
  - Probe: `conformance/probes/requested_list_read_at_compile_time.mojo`.
  - Found while landing R488 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R509 A constrained module constant is a plain value, where the pin
  makes it a generator type**

  Problem: `comptime C: Int where (f(1) > 5, "big only") = f(1)` with
  nothing reading `C` runs at the pin and stops in Mojito with "unsupported
  generic constraint operand".
  - At the pin a non-generic constrained constant has type
    `__generator_type Int where (...)`. Its clauses are checked only where
    a read instantiates it at `Int` (`var x: Int = C`).
  - The checker's `StmtKind::Comptime` arm (`checker/statements.rs`)
    compiles each clause (`compile_where_clause`) and validates it
    (`validate_declaration_constraint`) at the declaration, and
    `constraint_operand` (`checker/constraints.rs`) has no arm for a call.
  - Probe: `comptime C: Int where (1 > 5, "big only") = 1`, unread, stops
    with "constraint failed: big only", where the pin prints.
  - Probe: `print(C)` over `comptime C: Int where (1 < 5, "small") = 1`
    prints `1`, where the pin rejects it ("'__generator_type Int where
    True' does not conform to trait 'Writable'"): a Mojito-only
    acceptance.
  - Probe: `conformance/probes/constrained_module_constant_unread.mojo`.
  - Found while landing R488 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R519 A method of an unbound pack other than `__len__` has no
  symbolic rule**

  Problem: `a.__getitem__[0]()` over `*a: *Ts` is rejected with "no symbolic
  rule for an unbound pack or reflected type: method '__getitem__' of an
  unbound pack", where the pin runs it; `a[0]` is accepted.
  - Source validation left such a body without a verdict for a
    per-instantiation check that no longer exists; the one check judges
    every body with its binders symbolic
    (`tests/checker_test.rs::a_pack_body_without_a_symbolic_rule_is_rejected`).
  - The lever is a rule for the explicit dunder spelling beside the
    subscript's (`Checker::pack_element_type`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R472 A plain `def`'s `comptime for` element read through a `String`
  field materializes the whole element**

  Problem: `comptime for e in [Q(1, In(2), 1.5, "x")]:` then `print(e.s)`
  in `main` builds and destroys a `Q`, printing `del In 2 / del Q 1`, where
  the pin reads the field alone.
  - The AST unroller substitutes the element's construction for `e`, and
    folds a field chain to its leaf only when the leaf's literal has the
    field's type (`comptime/rewrite.rs::materializes_at_its_type`).
  - A `String`, float, `UInt`, or tuple leaf would materialize as a literal
    of another type, so `e.s` keeps reading the whole element.
  - An `Int`, `Bool`, `DType`, vector, or struct leaf folds (`e.a`,
    `e.i.v`, `show(e.i)`).
  - Found while landing R467 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R940 An `@inline` level that is not a spelled member is rejected**

  Problem: `comptime L = InlineLevel.never` then `@inline(L)` is rejected
  as unsupported, where the pin reads the operand as any compile-time
  `InlineLevel` value and runs the program.
  - Mojito reads only `.member` and `InlineLevel.member`
    (`InlineLevel::of`), since its prelude has no `InlineLevel` struct.
  - Add upstream's prelude `InlineLevel` to the bundled std and check the
    operand as a compile-time `InlineLevel` value, folding a closed one
    to the declaration's level.
  - `@inline(1)` should then fail with the pin's "cannot implicitly
    convert 'IntLiteral[1]' value to 'InlineLevel'".
  - Found while landing R913 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R941 An `@inline` level given by a parameter is rejected**

  Problem: `@inline(Self.policy)` on a method of
  `struct S[policy: InlineLevel]` is rejected, where the pin decides the
  level per instance and runs the program.
  - The declaration's `InlineLevel` holds only a closed level; it needs a
    parameter-expression form that MIR text and the package carry.
  - The elaborator then decides each instance's level, and
    `inline-parametric` treats an undecided level as `automatic`.
  - Found while landing R913 (2026-10-14).
  - Depends on R940.
  - Model: Opus, Not Planned.

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

- [ ] **R502 A parametric `comptime` alias in a function body is rejected**

  Problem: `comptime A[k: Int] = k + 1` inside a `def`, read as `A[10]`,
  fails with "a generic comptime alias must be declared at module scope",
  where the pin prints `11`.
  - The same alias at module scope runs.
  - Probe: `conformance/probes/local_generic_comptime_alias.mojo`.
  - Found while landing R310 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1478 A type comparison read as a value over a pack element at a
  `comptime for` index is not folded per iteration**

  `comptime for i in range(len(Ts)):` with `print(Ts[i] == Int)` or
  `comptime b = Ts[i] == Int` in `def h[*Ts: ...]` stops with "keeps the
  parameter constant `Int == Ts[...paramfor_next_value...($iter)]` in
  elaborated MIR", where the pin prints `True` and `False` for `h[Int,
  Bool]()`.
  - `comptime if Ts[i] == Int:` in the same loop runs: mono decides the
    condition's constraint form, but leaves the value form's `Const::Param`
    over the loop's iteration value open.
  - Probe: through a local alias, `comptime E = Ts[i]` then `print(E ==
    Int)`, fails the same way.
  - Found while landing R1380 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1484 A struct application's type member is no operand of a
  compile-time value**

  `comptime b = S[Int].A == Int` over `struct S[T: Movable]` with
  `comptime A = Int` fails with "unsupported associated comptime member
  access", where the pin prints `True`.
  - The one builder (`build_ct`, `checker/constraints.rs`) reads a
    type-valued operand only as a name, an application, or a type value;
    its `Member` arms read a value member, and `build_ct_rest` reports the
    rest.
  - Reading the member through `comptime_type_operand` there recurses
    back into `build_ct`; the type member's read needs the type position's
    resolver (`member_type_operand`) without the builder.
  - Probe: `materialize[S[Int].A == Int]()`, and over `struct S[*Ts:
    Movable]` `materialize[S[Int, Bool].A == Int]()` (R207's probe).
  - Found while landing R1388 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R402 A plain `def`'s `comptime for` over struct elements built by a
  call is rejected**

  Problem: `comptime for p in [mk(3), P(1)]:` in a non-generic `def` fails
  with "VM CTFE failed for 'mk': ... 'comptime for' over elements of type
  'P' in a generic body", where the pin prints each element.
  - The CTFE program for `mk(3)` still holds the enclosing loop, not yet
    unrolled, and its check rejects a loop variable that is no binder
    (`check_comptime_for`, `checker/comptime_validation.rs`).
  - The same loop in a generic `def` runs, on a clone.
  - Probe: `[mk(3), P(1)]` over `@fieldwise_init struct P` with
    `def mk(n: Int) -> P`.
  - Found while landing R363 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R444 A reflected field type over a closed subject in a plain `def`
  cannot be constructed**

  Problem: `comptime types = reflect[Holder].field_types()` in `main`, then
  `types[i]()` or `FT()` under `comptime if conforms_to(types[i],
  Defaultable & ...)` inside `comptime for i in
  range(reflect[Holder].field_count())`, fails with "type
  'type_sequence[Int, Float64][i]' does not conform to trait
  'Defaultable'", where the pin prints each field's default.
  - Source validation types the element of the closed list as a selection
    (`ParamKind::Select`), and `Checker::opaque_element`
    (`checker/comptime_validation.rs`) gives the arm's proof only to a pack
    element or a reflection over a parameter.
  - Past that, the AST unroller left `types[0]()` naming a dropped
    `comptime` binding ("Undefined variable 'types'"). R319's alias shape of
    that loss now runs, so re-probe this once the proof passes.
  - The same loop in a generic `def` runs, served by its template.
  - Found while landing R364 (2026-10-07).
  - Depends on R319, which loses a local alias at a call in an unrolled
    body.
  - Model: Opus, Not Planned.

- [ ] **R380 Two local `comptime` bindings of one display are different
  values in a type**

  Problem: `comptime L = [n, n * 2]` and `comptime M = [n, n * 2]`, then
  `var a: SIMD[DType.int32, L[0]] = SIMD[DType.int32, M[0]](1)`, in `def
  f[n: Int]()` is rejected with "type mismatch for variable 'a'", where the
  pin runs it.
  - The check denotes each binding by the application of its own lifted
    function, named by the binding (`record_display_binding`,
    `checker/comptime_validation.rs`), so `L[0]` and `M[0]` are two
    expressions.
  - The pin identifies the two reads by structure. It still tells `L[0]`
    from `n`, which `assets/type_error/comptime_display_element_identity.mojo`
    pins for `L[0] * 2` against `L[1]`.
  - The lever: bind each local display binding as its literal
    constructor's application (`Checker::display_application`), which
    identifies by structure.
  - Probe: `comptime x = (3, 4)` then `g[x]()` over `def g[t: Tuple[Int,
    Int]]()` in `main` fails with "not a compile-time Int constant: x",
    where the pin prints `7` for `t[0] + t[1]`. A bracket display
    (`g[(3, 4)]()`) already binds as the application.
  - Probe: `comptime L = [1, 2, 3]` then `f[L]()` in `main`, over `def
    f[c: Span[Int, _]]()` whose body is `comptime for i in range(len(c)):
    print(c[i])`, fails with "not a compile-time Int constant: L", where
    the pin prints `1 2 3`; `f[[1, 2, 3]]()` runs (found while landing
    R1712, 2026-10-30).
  - Found while landing a display binding read in a type or a parameter
    argument (2026-10-06).
  - Depends on R1529.
  - Model: Opus, Not Planned.

- [ ] **R390 Two spellings of an application are one value only when
  their names resolve to the same bindings**

  Problem: `comptime vals = [n, n * 2]` and `comptime other = [n, n * 2]`,
  then `var a: SIMD[DType.int32, h(vals[0])] = SIMD[DType.int32,
  h(other[0])](1)`, in `def f[n: Int]()` is rejected with "type mismatch for
  variable 'a'", where the pin runs it.
  - The check gives one lifted function to two occurrences of an expression
    when their syntax is equal and each name resolves to the same binding
    (`Checker::lifted_application`, `checker/comptime_validation.rs`).
  - The pin identifies the two by the structure of what they compute.
  - A tuple binding is the second case: `comptime t = (n, n * 2)` then
    `h(t[0])` outside a `comptime for` and inside it are two functions,
    since the binders a value that denotes no parameter expression reads
    are not known (`Checker::binders_read`).
  - The check still tells `h(n)` from `n * 2`, which
    `assets/type_error/comptime_application_identity.mojo` pins.
  - Found while landing an application in a type or a parameter argument
    (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1970 A local `comptime` value of a literal-typed call is lifted at
  `Int`, while its function returns the literal unconverted**

  `comptime y = (2).__add__(3); print(y)` fails "invalid checked program:
  MIR function '$comptime$main$c0' block 0: return of IntLiteral[5] from a
  function returning Int", where the pin prints `5`.
  - A literal dunder's application has no folded value (its body returns
    `{}` at an attribute-typed result), so the check lifts it
    (`Checker::lifted_application`, `checker/comptime_validation.rs`).
  - MIR's `request_applications` (`mir.rs`) declares the thunk at the
    application's type, a materialized `Int`, but lowers the call's
    `IntLiteral[5]` result as the return value with no conversion.
  - Upstream keeps the binding at the literal's own type and
    materializes it where it is used.
  - Probe: `comptime y = (2).__pow__(3); print(y)` fails the same way,
    where the pin prints `8`.
  - Probe: `comptime y = IntLiteral[(5).value](); print(y)` fails
    elaboration ("keeps the parameter constant `$comptime$main$c0()`"),
    where the pin prints `5`.
  - Found while landing R1855 (2026-11-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1208 A parameterized `comptime` value's declared type cannot read
  the member's own parameters**

  Problem: `comptime get[n: Int]: SIMD[DType.int32, n] =
  SIMD[DType.int32, n](7)` in `struct A`, read as `A.get[2]`, prints `[7,
  7]` at the pin, while Mojito stops with "not a compile-time Int constant:
  n".
  - The annotation resolves before the member's own parameters are in scope
    (`Checker::ct_member_req_from_anno`, `checker/constraints.rs`).
  - A trait requirement has the same gap: `comptime get[n: Int]:
    SIMD[DType.int32, n]` in `trait HasGet`, read through a bound as
    `T.get[2]`, prints `[5, 5]` at the pin.
  - The requirement's result type must then be substituted with the read's
    arguments (`Checker::generator_application`), and conformance must
    compare result types with the two binder lists aligned by position
    (`same_generator_signature`, `checker/traits_support.rs`).
  - Probe: `comptime get[T: AnyType]: T` in a trait is rejected with
    "unknown type 'T'", where the pin accepts the trait.
  - Found while landing R1146 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1178 A struct's type-valued parameterized `comptime` member is
  not read in a compile-time expression**

  Problem: over `struct W[T: AnyType]` with `comptime Get[i: Int] =
  Self.T`, `comptime E = W[Int].Get[0]`, `comptime if W[U].Get[0] ==
  Int:`, and a later member `comptime First = Self.Get[0]` are rejected,
  where the pin runs each.
  - Only a type annotation reads the member (`var x: W[Int].Get[0]`),
    through `Checker::resolve_parameterized_member`.
  - The one compile-time builder has no arm for it (`build_ct`,
    `checker/constraints.rs`), and every compile-time reader goes through
    it: `eval_associated_ct`, `constraint_operand`,
    `comptime_type_operand`, and a local `comptime` binding's.
  - The rejections differ by site: "function types as compile-time
    values", "Undefined variable 'W'", "unsupported dependent parameter
    expression", and "'Self' is not a compile-time type".
  - Probe: a struct member spelling a type pack's member out, `comptime
    First = Self.Ts.__getitem_param__[0]`, read as `var x: S[Int,
    Bool].First`, fails with "not an associated comptime expression".
  - Probe: the subscript spelling in a local `comptime`, `comptime T =
    TypeList[Trait=AnyType, Ts.values]()[1]`, fails with "type
    'TypeList[AnyType, Ts]' cannot be indexed here", where the pin runs it.
  - Probe: a later member applying a sibling's parameterized type member,
    `comptime F = Self.E[Self.n]` (or `Self.E[2]`) beside `comptime E[k:
    Int] = Box[k]`, read as `var b: S.F`, fails with "not an associated
    comptime expression", where the pin runs it; `var b: Self.E[Self.n]`
    in a method runs.
  - Found while landing R1168 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1716 A spread of a `ParameterList` value binds only a literal
  construction**

  `_ParameterListIter[*pl](0)` over `comptime pl =
  ParameterList[vs.values]()` in `def f[*vs: Int]()` fails with "cannot
  infer type parameter 'type'", where the pin runs it.
  - `Checker::parameter_list_spread` (`checker/comptime_validation.rs`)
    binds a variadic value parameter only from a spelled
    `*ParameterList[...]()` construction, not from any compile-time value
    whose type is a `ParameterList` application.
  - Upstream's spread reads the operand's type: the list is the
    application's `values` argument, wherever the value comes from.
  - Found while landing R333 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1149 A raw parameter list has no runtime form**

  Problem: `var x = vs.values` is rejected ("a raw parameter list
  ('KGENParamList[Int]') read at run time is not supported"), where the pin
  materializes the list as a value of type `KGENParamList[Int]`.
  - `TyKind::ParamList` has no layout, no VM value, and no native
    representation; the checker rejects every runtime read
    (`comptime_validation::raw_list_runtime_read`), and elaborated MIR
    rejects a register, slot, or signature that holds one
    (`verify/concrete.rs`).
  - Probes: a `comptime raw = vs.values` then `var x = raw`, and a runtime
    read of a `raw: _MLIR.KGENParamListType[t]` parameter.
  - Found while landing R807 (2026-10-19).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1638 A run-time call or read of a `!kgen.deferred` value is
  rejected by the checker, where the pin runs what its pipeline folds away**

  `_ = attr()` in `main`, over `def attr() -> __mlir_type.`!kgen.deferred``,
  stops with "failed to convert call result type: … only a compile-time
  position may call", where the pin prints `ran`.
  - The pin rejects `!kgen.deferred` only in LLVM lowering, on a function
    or call that survives inlining and DCE. Mojito already makes that
    rejection in `verify::llvm_translation_findings` (R1630), so the
    checker's two rules are a subset restriction on top of it.
  - Lever: delete `kgen_deferred_runtime_read`,
    `kgen_deferred_runtime_call`, and `deferred_result_bodies` (the
    checker), and make `!kgen.deferred` a
    `trivially_register_passable_leaf` (`mojito-types/src/types.rs`) so
    `mem-2-reg` promotes `_ = attr()`'s slot.
  - A 2026-10-29 prototype of that lever ran the first four probes and
    rejected the sixth as the pin does.
  - Probe: `_ = attr()` and `var a = attr()` in `main`, which print `ran`
    at the pin.
  - Probe: `_ = pick(n > 5)` over a run-time `n`, where `pick(b: Bool)`
    branches between two attributes, which prints `ran` at the pin.
  - Probe: `var a = __mlir_attr.`#pop.fmf<none>`` in `main`, which prints
    `ran` at the pin.
  - Probe: `print(take(__mlir_attr.`#pop.fmf<none>`))` over `def take(a:
    __mlir_type.`!kgen.deferred`) -> Int: return 7`, with and without
    `@no_inline`, which prints `7` at the pin. The prototype left a dead
    `const` of the attribute (and its drop) in `main`, which `canonicalize`
    does not erase and the pin's DCE does.
  - Probe: `var a = pick(n > 5)` where `pick` also prints, then
    `print(take(a))`: the pin rejects it with "failed to convert func
    signature", since the printing callee survives.
  - Probe: a local `comptime t = __mlir_attr[`#kgen.target<…>` :
    !kgen.target]` in `main`, read as `stdlib_plugin[t]()`, stops with
    "failed to legalize operation defining %r0 of type
    `__mlir_type.`!kgen.target``", where the pin prints the target's
    plugin name: the binding leaves a dead `!kgen.target` constant in
    `main`, as the deferred values above do. A module `comptime t` runs.
  - `assets/type_error/mlir_deferred_runtime_call.mojo` stays rejected, then
    by translation legality ("failed to convert call result type").
  - Update `docs/features.md`'s `__mlir_op` row, whose run-time-call and
    "run-time attribute local" clauses this entry retires.
  - Found while planning R1630 (2026-10-29).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R2038 Calling a function value whose variadic type pack is unbound
  stops with an internal error, where the pin rejects the call**

  `var g = f[3]` over `def f[n: Int, *Ts: AnyType](*args: *Ts) -> Int`,
  then `g(1, 2)`, stops with "MIR function 'main' variable slot 0 names
  parameter `*Ts` of `f` that no enclosing declaration binds".
  - The pin binds the unbound pack empty when it reads `f[3]` as a value,
    then rejects the call: "invalid indirect call: expected 0 elements in
    variadic pack, got 2 argument values".
  - Mojito keeps the pack open in the value's type, and MIR's checked
    handoff validation catches it.
  - Probe: the same over `llvm_intrinsic["llvm.fma", T,
    has_side_effect=False]` read as a value.
  - Found while landing R1521 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1193 A member read on a struct applied to a forwarded pack names
  the struct's own pack**

  Problem: `S[*Ts].n` and `TypeList[S[*Ts]._types].length` in `def f[*Ts:
  AnyType]` stop with "parameter constant names parameter `Ts` of `S` that
  no enclosing declaration binds", where the pin prints `3`.
  - `struct_member_at`'s `bind_declared` (`checker/constraints.rs`) binds a
    forwarded pack slot (`TyArg::Ty(pack)`) as a type, never as the
    `Self.Ts` list the member reads, so the member keeps `S`'s binder.
  - `S[Int, Bool, Int].n` runs.
  - Probe: `struct S[*Ts: AnyType]` with `comptime n =
    TypeList[Self.Ts.values].length`, read as `S[*Ts].n` in `f` and called
    `f[Int, Bool, Int]()`.
  - Found while landing R1184 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R2023 A type annotation does not read a member through a local
  compile-time value**

  `var a: w.E = True` over `comptime w = W[3]()`, where `W` declares
  `comptime E = Bool`, fails with "expected a type, found a value", where
  the pin prints `True`.
  - The pin types the annotation's base as an expression and reads the
    member on its type, as a member read through a construction
    (`W[3]().E`) already does here.
  - `Checker::member_base` (`checker/type_resolution.rs`) takes a
    `SourceType::Value` construction or a type, and resolves a bare
    `SourceType::Named` base as a type, so a local compile-time value's
    name is no base.
  - Probe: a parameterized member through a `TypeList` value, `var m:
    L.__getitem_param__[SIMDLength(0)] = 3` over `comptime L =
    TypeList.of[Trait=AnyType, Int, Bool]()`, fails with "unknown type
    'L'", where the pin prints `3`.
  - Found while landing R1540 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1473 A member read through a construction that takes arguments is
  rejected**

  Problem: `g[S(3).n]()` over `comptime n = 7` in a `@fieldwise_init struct
  S` fails with "a compile-time construction takes no arguments", where the
  pin prints `7`.
  - `member_type_operand` reads a member through a construction on the
    type `compile_time_value_type` (`checker/type_resolution.rs`) gives it.
  - That function accepts only a construction with no arguments, since a
    type subscript needs the value itself.
  - Upstream types the construction as a call and reads the member on its
    result type, without evaluating it.
  - The lever: a member read types the construction through call inference
    in the compile-time position.
  - Probe: a struct member's body, `comptime m = S(3).n`, fails the same
    way.
  - Probe: a type member read the same way, `comptime Cur2 = U(4).plugin`
    over `comptime plugin: Hooks = DefHooks[Self(3)]` in a `@fieldwise_init
    struct U`, fails the same way, where the pin reads `Cur2.lowers` as
    `True`.
  - Found while landing R1386 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1195 A struct `comptime` member's body is not checked against its
  annotation**

  Problem: `comptime N: Int = True` in a struct prints `True` for `S.N`,
  where the pin rejects it with "cannot implicitly convert 'Bool' value to
  'Int'".
  - `check_struct_associated` (`checker/constraints.rs`) evaluates the
    body and keeps the annotation as the member's type
    (`StructInfo::associated_value_types`).
  - It checks only a raw-list body against its annotation
    (`check_raw_list_member`).
  - R485 is the local `comptime` sibling, in `checker/statements.rs`.
  - Probe: a trait requirement's default, `comptime K: Int = "x"`, is
    accepted at the trait, where the pin rejects it there with "cannot
    implicitly convert 'StringLiteral["x"]' value to 'Int'"; a conformer
    taking it fails later with "comptime member 'K' does not match the
    requirement".
  - Probe: an `index` body under an `Int` annotation, `comptime n: Int =
    __mlir_attr[`#kgen.param_list.size<:`, …, `> : index`]`, is accepted
    and fails later at `print(L[Ts.values].n)` (not `Writable`), where the
    pin rejects the member with "cannot implicitly convert
    '__mlir_type.index' value to 'Int'".
  - Found while landing R1184 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1197 The elaborator cannot decide a pack's `all` or `any` through
  a predicate alias**

  Problem: `comptime if TypeList[Ts.values]().all[IsSmall]():` over
  `comptime IsSmall[T: AnyType] = IsTriviallyCopyable[T]` stops with
  "unsupported comptime if condition `PackPredicate { .. predicate:
  Alias("IsSmall") .. }` the instance does not decide", where the pin
  prints `1 0` for `f[Int, Bool](), f[Int, String]()`.
  - An alias's body does not reach MIR, so `native::mono` has no
    per-element proposition to evaluate.
  - A member clause `where TypeList[Self.Ts.values]().all[IsSmall]()`
    stays undecided the same way, and the checker's verdict at the call
    stands (`docs/notes/generator-contract.md`).
  - The builtin `IsTrivially*` predicates are decided.
  - Found while landing R1184 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1217 A parameterized member read on a concrete struct in a
  substituted signature does not close in the checker**

  Problem: `def f[T: Tr](x: Box[T.get[0]])` over `comptime get[idx: Int]:
  Int`, called as `f[S](Box[1]())` where `S` defines `get[idx: Int]: Int =
  idx + 1`, fails with "type mismatch for argument to 'f': expected
  Box[S.get[0]], found Box[1]", where the pin prints `1`.
  - `Checker::answer_concrete_members` (`checker/constraints.rs`) answers
    only member reads without arguments, so `S.get[0]` stays a read.
  - The struct side of the answer exists (`struct_member_at`,
    `ConformanceFacts::parameterized_associated`). It needs the member's
    parameters bound to the read's arguments, as the elaborator's
    `type_member_answer` (`native/mono/symbolic.rs`) does.
  - Found while landing R1201 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1218 A bound's value member in a struct parameter's type is not
  answered at the instance**

  Problem: `struct W[T: Tr]` with `var b: Box[Self.T.K]` and an initializer
  taking one, used as `W[S](Box[3]())`, fails with "type mismatch for
  argument 1 to 'W.__init__': expected Box[S.K], found Box[3]", where the
  pin prints `ok`.
  - The initializer's parameter type at `W[S]` substitutes `T` alone
    (`substitute_at`, `checker/indexing.rs`), which leaves `S.K`
    unanswered; it needs `resolve_assoc_ty`, as the instance method
    parameter paths in `method_calls/` take.
  - The same type as an instance method's parameter runs.
  - Probe: `trait Tr` with `comptime K: Int`, `struct S(Tr)` with `comptime
    K: Int = 3`, and `struct Box[n: Int]`.
  - Found while landing R1201 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1219 A static method's parameter reading its struct's bound
  value member stops the elaborator**

  Problem: `struct W[T: Tr]` with `@staticmethod def make(var b:
  Box[Self.T.K]) -> Int`, called as `W[S].make(Box[3]())`, stops with
  "unsupported monomorphizing `W.make`: 'T.K' is not a compile-time
  constant here", where the pin prints `4`.
  - The check accepts the call (`method_calls/statics.rs` closes `S.K` at
    the receiver), and the same parameter on an instance method runs.
  - `infer_call` (`native/mono/infer.rs`) reaches `W.make` with no
    instantiated arguments and a declaration holding none of the struct's
    parameters, so nothing binds `T` before the parameter's `T.K` is read;
    an instance method binds `T` from its receiver.
  - A static's body reading `Self.T.K` runs, so the gap is the parameter
    pattern only.
  - Probe: `trait Tr` with `comptime K: Int`, `struct S(Tr)` with `comptime
    K: Int = 3`, `struct Box[n: Int]`, and the `make` above returning `4`.
  - Found while landing R1201 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1200 A raw-list value parameter does not bind as a `TypeList`'s
  `values`**

  Problem: `def h[raw: _MLIR.KGENParamListType[AnyType]]() -> Int: return
  TypeList[Trait=AnyType, raw].length`, called as `h[Ts.values]()`, fails
  with "a parameterized type is not a value", where the pin prints `2`.
  - Probe: a signature over the binder, `ref x: TypeList[Trait=AnyType,
    raw].__getitem_param__[id]`, fails with "expected KGENParamList[AnyType],
    found a non-materializable compile-time value", where the pin compiles
    it.
  - R1184 found that `MetaTy::value` erases a raw list's trait element,
    which is the likely cause: the binder's list and the `values` slot's
    `KGENParamList[Trait]` do not meet.
  - Probe: a struct member's list of types over its own parameter,
    `comptime _cases = TypeList.of[Trait=Copyable, Int, Self.T].values`,
    fails conformance to `comptime _cases:
    _MLIR.KGENParamListType[Copyable]` ("does not match the requirement"),
    where the pin prints `2` for `TypeList[O[Int]._cases].length`. Its
    meta-type is `MetaTy::type_list()`, which keeps no trait, so
    `Checker::open_type_list_satisfies` (`checker/traits.rs`) accepts only
    an element every type conforms to (`AnyType`).
  - Found while landing R1185 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1490 A pack named in a parameter's type through its `.values`
  is neither inferred nor substituted**

  `def show[*Us: Movable](l: TypeList[Trait=Movable, Us.values]) -> Int`
  binds `Us` only from closed bracket arguments, where the pin also infers
  it from the argument's type and substitutes an explicit spread.
  - Probe: `show(Ts)` in `def go[*Ts: Movable]` reports "expected
    TypeList[Movable, ()], found TypeList[Movable, Ts]": unifying the
    argument's raw list with `Us.values` binds nothing. The pin prints
    `2` for `go[Int, Bool]()`.
  - Probe: `show[*Ts](TypeList[Ts.values]())` there reports "expected
    TypeList[Movable, Us], found TypeList[Movable, Ts]": the spread binds
    `Us` but does not reach the `Us.values` inside the parameter's type.
    The pin prints `2`.
  - `show[Int, Bool](TypeList.of[Trait=Movable, Int, Bool]())` runs.
  - Found while landing R1169 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1199 A trait method's signature cannot read a struct's value
  member**

  Problem: `def get(self, x: Q[P[Int, Bool].n]) -> Int: ...` in a trait
  fails with "not a compile-time Int constant: unsupported associated
  comptime member access", where the pin compiles it.
  - The same annotation in a struct method or a `def` runs.
  - The trait's abstract signature evaluates its bracket values without
    the struct table the other signature paths consult.
  - Found while landing R1185 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1174 A parameter typed by an imported package struct's
  `!kgen.type` alias is a value parameter**

  Problem: `def k[T: _MLIR.KGENTypeType]()` in a user module stops with
  "type mismatch for value parameter 'T': expected a value, found a type",
  where the pin binds `k[Int]()` and `k[_MLIR.KGENTypeType]()` and prints
  `2` for each.
  - `fold_kgen_type_parameters` (`comptime/synth.rs`) makes a
    `!kgen.type`-typed parameter a `__TypeOfAllTypes` type binder before
    any phase classifies it, but sees only the program's own structs'
    member aliases.
  - `prepare_importing` reads an installed package's module constants
    (`InstalledInterfaces::elaborated`) and no package struct's members, so
    the bundled `_MLIR`'s alias is folded only in the library's own build.
  - Probe: `conformance/probes/kgen_type_imported_member_alias.mojo`.
  - Probe: a `!kgen.string` member alias of an imported package's struct
    (`s: M.KS`), which the same fold spells as a value binder only for the
    program's own structs.
  - Found while landing R1150 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1176 An infer-only trait parameter is not inferred from the types
  it bounds**

  Problem: `g[Int, Bool, String]()` over `def g[Tr: type_of(AnyType), //,
  *Ts: Tr]() -> Int` prints `3` at the pin and stops with "cannot infer type
  parameter 'Tr' of 'g' from the arguments", as do `S[Int, Bool]` over
  `struct S[Tr: type_of(AnyType), //, *Ts: Tr]` and `f(3)` over `def f[Tr:
  type_of(AnyType), //, T: Tr](x: T)`.
  - The pin infers `Tr` as the join of the arguments' metatypes: one
    argument gives its own struct metatype (`AnyStruct[Int]`, or
    `AnyStruct[IntLiteral[3]]` from `f(3)`), several give the traits every
    one conforms to, and none fails to infer.
  - Mojito has no trait value for a struct metatype (upstream's
    `StructMetaType` converting to `AnyTrait`, `ExprConversions.cpp`), and
    no conformance join over the known traits.
  - Probe: `comptime T = type_of(Int)` runs at the pin (`AnyStruct[Int]`).
  - Probe: `count[MkList]()` over `def count[G: __generator_type[T:
    Copyable] Defaultable]()` and `comptime MkList[T: Copyable] = List[T]`
    binds at the pin, which types `MkList` read unapplied as
    `__generator_type[T: Copyable] AnyStruct[List[T]]`. Mojito types it
    with an `AnyType` result and rejects the binding. The body's
    `G[Int]()` already checks as the construction.
  - Probe: `width[MkList]()` over `def width[G: __generator_type[T:
    Copyable] Copyable & Defaultable]() -> Int` returning
    `size_of[G[Int]]()` binds at the pin, where Mojito rejects the
    `AnyType`-result generator the same way.
    Mojito says "'type_of' of a type is not supported yet"
    (`Checker::type_of_operand`, `checker/constraints.rs`).
  - Upstream's stdlib relies on it in `TypeList.of[...]` without `Trait=`
    (`std/builtin/bool.mojo`, `std/collections/optional.mojo`).
  - Probe: `TypeList.of[Bool, Int]()[1]` as a local annotation stops with
    "cannot infer type parameter 'Trait' of 'TypeList[Trait, values].of'",
    where the pin prints the value; the alias's `Trait` is this binder, and
    the pin's diagnostic for `TypeList.of[Int]().all[P]()` names the solved
    `AnyStruct[Int]`.
  - Probes: `var w: W[AnyType] = g[Int]()` over `-> W[Tr]` reports
    "cannot implicitly convert 'W[AnyStruct[Int]]' value to 'W[AnyType]'"
    at the pin, `g[Int, Bool]()` reports `W[Boolable & Comparable &
    ...]`, and `g[]()` "failed to infer parameter 'Tr'".
  - A body's use of `T: Tr` against a bound `Tr` does not imply (`need[Ts[0]]()`
    over `Tr: type_of(AnyType)`) names the trait parameter at the pin ("has
    'Copyable' type, but value has type 'Tr'") and the element in Mojito.
  - Found while landing R1167 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1695 A value binder typed by a trait parameter is not inferred,
  aliased, or read through**

  `C[default]` over `struct C[Trait: type_of(AnyType), //, D: Trait]`,
  where `default: Trait` is itself such a binder, stops with "cannot infer
  type parameter 'Trait' of 'C'", where the pin solves `Trait` as the
  binder's own trait parameter.
  - Unlike R1176, the argument is a binder whose declared type is the
    trait parameter, not a struct's metatype, so no conformance join is
    needed.
  - Probe: `comptime first[Trait: type_of(AnyType), *, default: Trait] =
    default`, then `StaticString(first[StdP, default=DefP].name)`, stops
    with "Undefined variable 'first'", where the pin prints `default`.
  - Probe: `first[DefP].HookFor[2]` over `comptime first[default: StdP] =
    default` reports "type 'DefP' has no field 'HookFor'".
  - Probe: `comptime X: StdP = first[StdP, DefP]` reports "unknown type
    'StdP'".
  - Probe: `conforms_to(X, Self.Trait)` over such a binder, which
    `TargetPluginSelector._find` reads (R1694).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1696 A `TypeList` member read in a struct member or a module
  alias fails**

  `comptime n = Self.L.length` over `comptime L = TypeList.of[Trait=AnyType,
  Self.D]()` in `struct C[D: StdP]` stops with "not an associated comptime
  expression", where the pin prints `1` for `C[DefP].n`.
  - The same reads in a local `comptime` work.
  - Probe: `Int(mlir_value=Self.L._raw_length)` over the same list.
  - Probe: `Self.L.length` over `comptime L = TypeList.of[Trait=AnyType,
    Self.D, Int]` (the application, unconstructed) reports "unsupported
    associated comptime member access".
  - Probe: a module `comptime T0 = L._get_type_at_index[__mlir_attr.`0 :
    index`]` over `comptime L = TypeList.of[Trait=AnyType, DefP]`, then
    `L.length`, reports "expected a value, found Type".
  - `TargetPluginSelector.selected` and `_find` read `Self.Plugins`'s
    `_raw_length` and `_get_type_at_index` this way (R1694).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1700 A conditional expression over types is rejected**

  `comptime pick[b: Bool]: StdP = DefP if b else DefP` stops with "expected
  a value, found Type", where the pin binds the selected type
  (`StaticString(pick[True].name)` prints `default`).
  - The checker types `a if c else b` as a value expression only, so a
    type operand in either arm is rejected.
  - `TargetPluginSelector.selected` is this shape (R1694).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1697 `#kgen.param.expr<add>` over `index` is outside the subset**

  `find[__mlir_attr[`#kgen.param.expr<add,`, idx, `, 1 : index> : index`]]()`
  stops with "the attribute … is outside the subset", where the pin
  recurses through the parameter.
  - Lever: an `mlir_bracket_attribute` arm building the existing index
    addition `ParamOp`.
  - Probe: a `find[idx: __mlir_type.index]` that returns `idx` once
    `_index_lt[1, idx]()` holds and otherwise recurses on the sum, read as
    `comptime i = find[__mlir_attr.`0 : index`]()`, which the pin prints
    as `2`.
  - `PluginSelector._find` and `TargetPluginSelector._find` recurse this
    way (R1694).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1701 `__mlir_op.`llvm.intr.trap`` is outside the subset**

  `__mlir_op.`llvm.intr.trap`()` in `main` stops with "is outside the
  subset", where the pin traps at run time and fails the evaluation when
  compile-time evaluation reaches it.
  - Lever: map it to the abort the VM and Pliron already lower.
  - `TargetPluginSelector._find` traps past the list's end (R1694).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1177 A struct's trait parameter does not bound a method's type
  parameter**

  Problem: `def m[T: Self.Tr](self)` in `struct S[Tr: type_of(AnyType)]`
  runs `S[Copyable]().m[Int]()` at the pin and stops with "type 'S[Tr]' has
  no associated type 'Tr'".
  - The parser reads a `Self.X` bound as a value parameter typed by
    `Self.X`, and the checker's trait-parameter bound
    (`ParamDecl::Type::trait_param`) names only a sibling of the same list.
  - The use-site check (`Checker::validate_trait_parameter_bounds`) reads
    the trait from the declaration's own arguments, while this one is the
    receiver's struct argument.
  - Found while landing R1167 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1171 A trait is not a `comptime` binding**

  Problem: `comptime T = Copyable`, `comptime MyTr = Copyable & Movable`,
  and `comptime U: type_of(AnyType) = Movable` stop with "'Copyable' is not
  a compile-time type", where the pin binds each and passes it to a
  trait-valued parameter (`g[MyTr]()` over `g[Tr: type_of(Movable)]`).
  - Every form reaches the check, module or local: it reads `Copyable &
    Movable` as a value expression ("Undefined variable 'Copyable'") and a
    module `comptime T = Copyable` as an unknown type ("unknown type
    'Copyable'").
  - The lever: a trait-valued alias in `Checker::comptime_aliases` (an
    `AliasBody::Trait`, or a `CtValue::Trait` value), module or local, read
    by `Checker::trait_value` wherever a trait is spelled.
  - Using such an alias as a bound (`T: MyTr`) is R1167's bound form, not
    this entry's.
  - Probe: `comptime MyTr = Copyable & Movable`, then `print(g[MyTr]())`,
    prints `1` at the pin.
  - Probe: upstream's `std.collections.dict` declares `comptime KeyElement
    = Movable & Hashable & Equatable`, which `String`, `StringSpan`, and
    `Path` list as a conformance and `Dict` and `Set` take as a bound. The
    bundle spells the composition out and omits it from the lists, so a
    program naming `KeyElement` finds no declaration.
  - Probe: upstream's `std/utils/static_tuple.mojo` declares `comptime
    _StaticTupleTraits = ImplicitlyCopyable & Deinitable &
    RegisterPassable` and bounds `StaticTuple` and its construction check
    with it; the bundle spells the composition out per bound.
  - Found while landing R1166 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1173 A type or trait read as a run-time expression is rejected**

  Problem: `_ = Tr` over `Tr: type_of(AnyType)`, and `_ = T` over a type
  parameter, run at the pin, while Mojito stops with "unsupported runtime
  value parameter `Copyable`" and "Undefined variable 'T'".
  - A type value (upstream's `!kgen.type`) has no run-time form in Mojito:
    identifier inference (`Checker::infer_impl`) resolves a trait-valued
    binder to its metatype but lowers the read as a value-parameter
    register, and resolves a type name to nothing.
  - `Tr == Copyable` is rejected with "Undefined variable 'Copyable'"
    where the pin reports "'AnyTrait[AnyType]' does not implement the
    '__eq__' method": a trait name in expression position has no value
    either.
  - Found while landing R1166 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1649 A partially bound alias whose open parameter's name its bound
  arguments read is rejected**

  Problem: `comptime X = W[n, _]` over `struct W[m: Int, n: Int]` and a
  module `comptime n = 3` is rejected with "the unbound parameter 'n' of
  'W' shares its name with a name its bound arguments read", where the pin
  binds `X[4]` to `W[3, 4]`.
  - `Checker::partially_bound_generator` (`checker/constraints.rs`) spells
    the partial application as a generator alias whose binders are the
    struct's open parameters, by name, so a binder would shadow the module
    name the bound argument reads.
  - Upstream binds the open slots on the type itself, never in the alias
    body's scope: a struct type with open slots that an application fills.
  - Found while landing R296 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1746 A `comptime` alias of an unapplied parametric struct is
  not that struct's generator**

  `comptime Q = P` over `struct P[n: Int]` is rejected at its declaration
  with "cannot infer type parameter 'n' of 'P' from the arguments", where
  the pin binds `Q` to the unapplied `P`, applicable as `Q[2]` and `Q[_]`.
  - The alias's body is read as a value of `P`, so a struct whose
    parameters cannot be inferred fails there.
  - A struct whose parameters are all infer-only binds, but as the
    struct with no parameters: the bundled `comptime StringSlice =
    StringSpan` applied as `StringSlice[_]` or
    `StringSlice[ImmStaticOrigin]` is rejected with "type
    '__module$std$string$StringSlice' expects 0 type argument(s), got 1".
  - Probe: `def f(x: Q[_])` and `def g(x: Q[2])` over `comptime Q = P`,
    called with `P[2]()`, print `2 2`, here and at the pin.
  - Probe: `def f(x: StringSlice[_])` called with `StaticString("a")`
    prints `a`.
  - Probe: the bundled `String.__radd__` takes upstream's `other:
    StringSlice[_]` (`stdlib/std/string.mojo`), where it now takes the
    bare `StringSlice`.
  - Probe: the bundled `VariadicPack._write_to` cannot spell upstream's
    `start: StringSlice[O1]` (with R1926).
  - Found while landing R1653 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1704 A parametric function type spelled inline in a type does
  not resolve**

  `var o: OptionalReg[def[d: DType, w: SIMDLength, //](SIMD[d, w]) thin
  -> SIMD[d, w]] = None` reports "'d' does not name a DType in scope",
  where the pin prints `False` for `Bool(o)`.
  - The same type as a `comptime` alias body (`comptime _B = def[…](…)
    thin -> …`) or a callable parameter's bound resolves, through
    `Checker::lower_anonymous_callable_type`.
  - Lever: `Checker::ty_from_anno` takes `&self` and opens no scope for
    the function type's own `def[...]` binders. `push_param_scope` and
    `classify_params` need `&mut self`.
  - `Checker::generator_type` has the same gap for a generator type's own
    list: it resolves a result naming only one binder bare (`T`).
  - Probe: `def f[G: __generator_type[T: AnyType] List[T]]()` reports
    "not a compile-time Int constant: T", where the result is `List[T]`
    over the generator type's own binder.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1707 A function is no compile-time value of a function type**

  A struct's `comptime f: OptionalReg[def(Int) thin -> Int] = sq`, read as
  `comptime (D.f.value())(5)`, stops with "not a compile-time Int constant:
  sq", where the pin prints `25`.
  - Probe: the same member spelled `= OptionalReg[def(Int) thin ->
    Int](sq)`, read in `comptime if D.f:`, stops with "not an associated
    comptime expression", where the pin prints the then arm.
  - `CtValue` has no value for a function symbol, so neither the member
    nor its `OptionalReg` construction is a parameter expression.
  - A plugin that sets a `PluginHooks` math hook (`unary_math_fn`) needs
    this. The default hooks are `None` and already read.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1487 A generator-typed generator parameter is not supported**

  Problem: upstream's
  `test/mojo-integration/generator_param_applied_to_earlier_params.mojo`
  declares `GT: __generator_type[From: type_of(AnyType), To:
  type_of(AnyType)] type_of(__generator_type[From: From] To)` and `G: GT[F,
  T]`, and runs.
  - The result of `GT` is itself a generator type, applied in a later
    parameter's type: R1486's type-position application, over a
    `type_of(...)` of a generator type, which `type_of` rejects as "of a
    type" (R1176's metatype).
  - No bundled library use needs it yet.
  - Found while landing R808 (2026-10-27).
  - Depends on R1486.
  - Model: Opus, Not Planned.

- [ ] **R1488 A trait's parameterized value requirement is not a
  generator-typed value requirement**

  Problem: upstream types `comptime get[idx: Int]: Int` as a value
  requirement of type `__generator_type[idx: Int] Int`, where Mojito keeps
  a requirement form of its own, `CtMemberReq::Generator { params, result
  }`, beside `CtMemberReq::Value`.
  - `TyKind::Generator` now types a generator value and a generator
    parameter; folding the requirement into
    `CtMemberReq::Value(Ty::generator(..))` gives all three one type, and
    witness matching becomes `generator_converts`.
  - `Checker::lookup_trait_generator` and the witness comparison in
    `traits.rs` are the readers to move.
  - Found while landing R808 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R396 A signature that converts a computed value or calls a method
  on a struct value is rejected**

  Problem: `def gen[n: Int]() -> SIMD[DType.int32, Int(half(n))]:` for `def
  half(x: Int) -> Float64` is rejected with "not a compile-time Int
  constant: 'Int' conversion of a symbolic 'Float64' value", and `->
  SIMD[DType.int32, P(n).get()]` for a method `get` of `struct P` with "not
  an associated comptime expression", where the pin runs both.
  - The builder's scalar-conversion arm (`build_ct`,
    `checker/constraints.rs`) re-reads an operand already of the target
    type, where upstream applies the target's `__init__` to it.
  - A method call's receiver is a struct value, which no applicable
    function takes (`applicable_functions`,
    `checker/comptime_validation.rs`).
  - In a body each of these is lifted whole, so it runs there, as a
    different value from the same call spelled in another declaration.
  - A struct value in the parameter domain can take the frozen form a
    struct-typed parameter argument already has.
  - Probe: `S[u.__mlir_index__()]` over a binder `u: UInt` (`S[n:
    __mlir_type.index]`) fails with "not an associated comptime
    expression", and `Int(u)` in the builder with "'Int' conversion of a
    symbolic 'UInt' value", where the pin runs both: upstream's
    `SIMD.__mlir_index__` is `pop.cast_to_builtin` of `pop.cast` to `int`.
    A closed `UInt(3).__mlir_index__()` folds, but `Int(UInt(3))` in a
    type argument is still "not a compile-time Int".
  - Probe: `def h[F: Float64, G: Bool = Bool(F)]()` is rejected with
    "'Bool' conversion of a symbolic 'Float64' value", where the pin prints
    `False True` for `h[0.0]()` and `h[2.0]()`; `F.__bool__()`, which
    `Bool.__init__` calls, already builds as an application.
  - Found while landing the callee shapes a signature applies (2026-10-06).
  - Depends on R386, which gives the parameter domain a value meta for a
    `Float64` and a `String`.
  - Model: Fable, Not Planned.

- [ ] **R2043 An `Array` display's constant subscript in a signature is no
  application, as the bundled `Array` has no `__getitem_param__`**

  `def f(x: SIMD[DType.int8, [2, 1][0]]) -> Int: return len(x)` called
  with `SIMD[DType.int8, [2, 1][0]](1, 2)` is rejected with "not an
  associated comptime expression", where the pin prints `2`.
  - At the pin a constant subscript of an `Array` is
    `load_from_mem` of its `__getitem_param__[idx]()` application.
  - A subscript in a compile-time position is its getter's application
    (`selected_application_at`, `checker/comptime_validation.rs`), but
    the getter is declined for an `Array` receiver: spelling it
    `__getitem__` would accept a signature `[2, 1][0]` called with
    `[2, 1].__getitem__(0)`, which the pin rejects as two applications.
  - The fix: once R511 gives `Array` its `__getitem_param__` and routes a
    constant subscript there (`Checker::subscript_getter`,
    `checker/indexing.rs`), drop the guard.
  - Probe: the program above: `2` at the pin.
  - Probe: `assets/type_error/comptime_display_subscript_in_signature.mojo`
    should expect the pin's `.__getitem_param__[0]()` mismatch message.
  - Probe: a signature `[2, 1][0]` called with `[2, 1].__getitem__(0)`:
    rejected at the pin ("cannot be converted from ...[Int(0)] to
    ....__getitem_param__[Int(0)]()"); pin it as a type_error fixture.
  - Probe: `struct S[n: Int]: comptime w = [Self.n, 1][0]` with
    `print(S[2].w)`: `2` at the pin.
  - Found while landing R1990 (2026-11-08).
  - Depends on R511.
  - Model: Fable, Not Planned.

- [ ] **R387 A function applied in a parameter argument cannot print**

  Problem: `g[noisy(n)]()` in `def f[n: Int]()`, where `noisy` prints before
  it returns, is rejected with "is not safe for VM-backed compile-time
  execution: it reaches `print`", where the pin prints once while compiling
  and runs.
  - The elaborator refuses to run a lifted function that reaches `print` or
    `input` (`effectful_callee`, `mono/specializer.rs`).
  - The pin's interpreter writes compile-time output when the instance is
    elaborated.
  - Decide where an elaboration-time `print` writes on each backend before
    lifting the refusal.
  - Found while landing an application in a type or a parameter argument
    (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R388 A type over a lifted application is spelled by its function's
  name in a diagnostic**

  Problem: `var a: SIMD[DType.int32, max(n, 2)] = SIMD[DType.int32, n *
  2](1)` in `def f[n: Int]()` is rejected with "expected SIMD[DType.int32,
  $comptime$$2014(n)], found SIMD[DType.int32, 2 * n]", where the pin spells
  the expected type `SIMD[.int32, max(n, Int(2))]`.
  - Both compilers reject the program. Only the spelling differs.
  - The application names the function MIR lifts, not the expression it
    computes (`Checker::lifted_application`,
    `checker/comptime_validation.rs`).
  - A call of a plain module `def` is spelled as written (`h(n)`), since it
    is the application of that `def`.
  - Found while landing an application in a type or a parameter argument
    (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R398 A type over the application of a generic, overloaded, or
  static callee is spelled by its symbol in a diagnostic**

  Problem: `var x: SIMD[DType.int32, 4] = gen[2]()` for `def gen[n: Int]()
  -> SIMD[DType.int32, twice[n]()]` is rejected with "found
  SIMD[DType.int32, twice(2)]", where the pin spells the found type
  `SIMD[.int32, twice[Int(2)]()]`.
  - Both compilers reject the program. Only the spelling differs.
  - An overloaded callee prints its symbol: `ov$ov$Int(2)` for `ov(2)`.
  - A library application prints its overload's symbol and every
    defaulted argument: `size_of[Pair]()` in a type is
    `__module$std$sys$info$size_of$ov$$ct$type$target[Pair](…)`, where the
    pin elides the defaulted `target` and prints `size_of[Pair]()`
    (`assets/type_error/comptime_layout_constant_mismatch.mojo`).
  - A static method of a generic struct prints the instance it is called on
    as an argument: `G.w[G[2]]()` for `G[2].w()`.
  - `not (n > 9)` prints as `not 9 < n`.
  - The pin prints an `@implicit` constructor's application as its
    argument: `var x: T[W(3)] = T[W(4)]()` over `struct T[w: W]` is
    "cannot implicitly convert 'T[Int(4)]' value to 'T[Int(3)]'", where
    Mojito prints `T[W(4)]`.
  - A local `comptime L = W(n)` alias of a struct value prints as the
    application of its lifted function, `B[$comptime$f$c0(n)]`, where the
    pin prints `B[L]` and notes "'L' is aka 'W(n)'".
  - A method's application prints its receiver as a first argument:
    `var x: SIMD[DType.int8, 4] = f[2]()` over `def f[n: Int]() ->
    SIMD[DType.int8, L(n).ord()]` finds `SIMD[DType.int8, L.ord(L(2))]`,
    where the pin names `L(Int(2)).ord()` (found while landing R1643,
    2026-10-30).
  - The application holds the MIR declaration's name and every argument in
    one list (`ParamKind::Apply`, `param_expr.rs`), so the printer knows
    neither the source name nor which arguments are compile-time ones, nor
    which one is a method's receiver.
  - Found while landing the callee shapes a signature applies (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R383 A vector width that mismatches a display element is reported as
  a missing constant**

  Problem: `comptime vals = [n, n * 2]` then `var a: SIMD[DType.int32,
  vals[0]] = SIMD[DType.int32, n](1)` in `def f[n: Int]()` is rejected with
  "not a compile-time Int constant: vals", where the pin reports that
  `SIMD[.int32, n]` does not convert to the annotated type.
  - Both compilers reject the program. Only the diagnostic differs.
  - Source validation reports it, on the conversion it tries after the two
    types differ.
  - The capitalized spelling `L[0]` reports "SIMD width must be a positive
    integer, got a type" instead.
  - Found while landing a display binding read in a type or a parameter
    argument (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R448 A value that solves a type parameter to `String` is rejected**

  Problem: `f["lit"]()` and `f[String("s")]()` over `def f[T:
  ImplicitlyCopyable & Writable, //, v: T]() -> T` fail with "value
  parameter 'v' typed by 'T' cannot bind a 'String' value yet", where the
  pin solves `T = String` and prints `lit` and `s`.
  - A compile-time string is a `CtValue::Str`, typed `StringLiteral`, so a
    read of `v` at `T = String` would put a literal in a `String` register.
  - Mono must close the read as the literal followed by the conversion MIR
    already uses for a `[s: String]` read; the checker then accepts the
    `Str` against the `String` struct in one shared rule.
  - The rejection lives in `Checker::solve_binders_from_values`.
  - Probe: `ParameterList.of["foo", "bar"]` fails the same way, for
    `values` typed by `type`, where the pin solves `type = String` and
    prints `(foo, bar)` for its construction.
  - Found while landing R446 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1175 Compile-time bracket binding is re-implemented outside `call.rs`**

  Problem: four sites bind a bracket list to a declaration's parameters by
  their own rule instead of `mojito_ast::call::match_parameter_slots`, the
  pin's rule the checker's `bracket_bindings` uses.
  - `callable_value_argument` (`checker/origins/subst.rs`) binds positional
    arguments over infer-only binders, and skips a keyword-filled slot.
  - `applied_arguments` (`checker/comptime_validation.rs`) pairs a binder
    with the argument at its raw position.
  - `mir::supplied_parameter_arguments` (`mir/ir.rs`) is a separate copy.
  - `verify_param_arguments` (`mir/verify/subscripts.rs`) lets a
    positional argument skip a keyword-filled slot, which the pin rejects.
  - MIR lists carry erased origin arguments with no value, which the MIR
    adapters must keep aligned.
  - Found while landing R449 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R451 The elaborator classifies a value parameter whose type it cannot read as a type parameter**

  Problem: `classify_ct_param` (`comptime/params.rs`) makes `v` in `def
  pick[v: Self.T](self)` a type parameter bounded by "trait `Self.T`", and
  does the same for a value type it cannot read (`items: List[T]`), while
  the checker classifies both as value parameters.
  - Programs run today because a template-served method binds its value
    from the call's recorded arguments, not from this classification.
  - The method form needs the enclosing struct's binder identity, which
    `classify_ct_param` is not given.
  - Found while landing R446 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R803 A `where` clause over a value pack's length reads the pack as
  a type list**

  Problem: `def f[*vs: Int]() -> Int where len(vs) > 1`, called as
  `f[4, 5]()`, fails with "cannot prove constraint
  'TypeList[vs.values]().length > 1'", where the pin runs it.
  - `Checker::typelist_receiver` (`checker/constraints.rs`) takes every
    enclosing variadic parameter for a type pack, a value pack included.
  - The same cause stops `comptime if len(vs) == 2:` over `*vs: Int` in
    the elaborator with "unsupported comptime if condition
    `Eq(PackLength(...))` the instance does not decide", where the pin
    prints `202` for `f[1, 2]()` (found while landing R805, 2026-10-20).
  - Found while landing R494 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R804 The length of a value pack in a signature type is rejected**

  Problem: `def sized[*vs: Int]() -> Box[len(vs)]` fails with "not a
  compile-time Int constant: not an associated comptime expression", where
  the pin types the result by the pack's length.
  - A signature has no body to lift `len(vs)` from, and `len` is now the
    library function, but `called_application` serves only `Int` and
    `Bool` arguments, so a pack argument still needs its own path (R1284).
  - `Box[vs.size]` is the spelling that works today.
  - Found while landing R494 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1778 A function with a named `out` result is never a
  compile-time application**

  `h[mk(5)]()` over `def mk(a: Int, out result: B)` and `def h[b: B]()`
  prints `5` at the pin. Mojito reports "not a compile-time Int constant:
  not an associated comptime expression".
  - Cause: `ApplicableShape::applicable` (`checker/comptime_validation.rs`)
    drops any callable with an `out` parameter, so `called_application`
    finds no candidate, though `infer` types the call.
  - The application's result is the `out` parameter's type, as a `->`
    result's is.
  - Probe: `v.shuffle[mask=Index(3, 2, 1, 0)]()` on a `SIMD[DType.int32,
    4]`, through `Index`'s `out result: IndexList[args.__len__(), …]`;
    `comptime m = Index(…)` then `mask=m` runs.
  - Found while landing R1675 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2044 A value parameter read only through an application in
  another parameter's type is removed, and its caller fails to elaborate**

  `def use[v: SIMD[DType.int32, 4]](var t: T[f(v)]): print("ok")`, called
  as `use[v](T[f(v)]())`, fails with "unsupported monomorphizing `use`:
  unresolved value parameter `v`", where the pin prints `ok`.
  - `remove-unused-params` (`unread_parameters_of`, `mir/param_uses.rs`)
    walks the body, not the runtime parameters' declared types, so it
    drops `v`, which only `t`'s type reads.
  - `Specializer::infer_call` (`native/mono/infer.rs`) then unifies `t`'s
    pattern `T[f(v)]` through `substituted_struct_arguments` with `v`
    unbound.
  - Upstream's `RemoveUnusedParams` keeps a parameter the function's type
    depends on: count a parameter a runtime parameter's declared type
    reads as read.
  - Probes: the shape above over a free `f(x: SIMD[DType.int32, 4]) ->
    Int`; the same with `T[v.reduce_add()]` over `struct T[n: Int32]`
    (R1995's headline). Each runs once the body reads `v`.
  - Found while landing R1995 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1220 A `TypeList` proposition over a pack in a signature type is
  rejected**

  Problem: `def f[*Vs: AnyType](x: B[Vs.contains[Int]()]) -> Int` over
  `struct B[b: Bool]` fails with "not a compile-time Int constant: not an
  associated comptime expression", where the pin prints the result.
  - The builder reads the proposition as a `ParamKind::PackQuery` (R1901),
    but `ParamContext::replace_at` closes only `PackQuery::Length` against
    a bound pack, so the call reports "expected
    B[TypeList[Vs.values]().contains[Int]()], found B[true]".
  - `all_conforms_to[Tr]()`, `all[P]()`, and `any[P]()` are the same
    shape.
  - Probe: the same parameter on a trait requirement and its witness.
  - Found while landing R1198 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R490 A call that leaves a value pack empty is rejected**

  Problem: `total()` and `total[]()` over `def total[*values: Int]() ->
  Int` fail with "cannot infer type parameter '*values' of 'total' from the
  arguments", where the pin binds the empty list and prints `0`.
  - Probe: `none[]()` over `def none[*vals: Int]() -> Int` prints `0` at
    the pin.
  - The checker's generic call binding has no empty list for an unsupplied
    variadic value parameter.
  - A clone of such a `def` would spell its runtime reads as
    `ParameterList()`, whose infer-only `type` then has no value to solve
    from; the keyword form `ParameterList[type=Int]()` binds it.
  - Found while landing R325 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R452 A `comptime` alias of an applied generic function is rejected**

  Problem: `comptime g = f[7]` then `g()`, over `def f[v: Int]() -> Int`,
  fails with "not a compile-time value: 'f' is not a compile-time type",
  where the pin binds the specialized function and prints `7`.
  - The alias path evaluates the right-hand side as a type application.
  - With a type argument (`f[Int, 7]`) it fails with "unknown type 'f'".
  - Found while landing R446 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R463 A struct's tuple-typed value parameter cannot be read in its
  methods**

  Problem: in `struct T[p: Tuple[Int, Int]]`, `Self.p[1]` fails with
  "Undefined variable 'Self'", where the pin prints the element of
  `T[(1, 2)]`.
  - `var t = Self.p` then `t[1]` runs on both backends.
  - A `Tuple[Int, String]` parameter fails the same way.
  - A struct's scalar or struct-typed value parameter reads (`Self.n`,
    `Self.e.rows`).
  - Probe: `comptime n = len(Span(Self.l))` in `struct H[l: List[Int]]`
    is "not an associated comptime expression", where the pin prints `3`
    for `H[[4, 5, 6]].n`: typing the `Span` construction borrows `Self.l`
    and fails with "Undefined variable 'Self'". The same `Span(l)` over a
    `def` binder `l` applies.
  - Found while landing R455 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1715 A type pack read through a struct application or a value is
  no pack**

  `len(P[Int, Bool].Ts)` over `struct P[*Ts: Movable]` fails with "no
  overload matches" for `len`, where the pin prints `2`; `len(Self.Ts)`
  in `P`'s own method runs.
  - `Checker::type_pack_named` (`checker/comptime_validation.rs`) knows a
    type pack only as a binder in scope or as `Self.<pack>`.
  - A value parameter read the same ways is the application's argument
    (`Checker::struct_parameter_at`); a pack parameter needs the same read,
    giving the application's `TypeList`.
  - Probe: `len(r.Ts)` over `r: _RegisterPackType[SIMD[DType.int32, 4],
    SIMD[DType.int32, 4]]` fails the same way.
  - Probe: `comptime T2 = P[Int, Bool].Ts` then `len(T2)` fails with "a
    parameterized type is not a value", where the pin prints `2`.
  - Found while landing R333 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R65 Upstream `DType` names with no Mojito dtype are rejected**

  Problem: `print(DType.uint128)` runs at the pin (`uint128`), while Mojito
  reports "DType.uint128 is not supported yet"
  (`assets/type_error/dtype_upstream_only_rejected.mojo`, a `divergence` row of
  `conformance/assets-mojo-errors.tsv`).
  - The names are `bfloat16`, `int128`, `uint128`, `int256`, `uint256`, the
    `float4`/`float6`/`float8` families, and `_uint1`/`_uint2`/`_uint4`
    (`UPSTREAM_ONLY_DTYPE_NAMES` in `mojito-ast`).
  - `Dtype::UInt` (R1389) is the latest dtype added, with every table row
    and Pliron's predicate arms included.
  - The declared float queries `max_exponent` and `exponent_width` in
    `stdlib/std/builtin/dtype.mojo` answer only `float16`/`float32`/
    `float64`; a new float format restores its upstream arms there.
  - `llvm_intrinsic::fpclass_lane`'s smallest-normal match has a wildcard
    arm, so a new float format adds its row there by hand.
  - Each needs a `Dtype` variant with upstream's code in `Dtype::code`, even
    where no `SIMD` lane of it exists yet, so the value can print and compare.
  - `Float16` set the pattern for a new lane: extend the table methods in
    `mojito-ast`, route rounding through `Dtype::round_lane` and
    `Dtype::float_literal_lane`, and let the build's exhaustiveness errors
    list the rest. The native lowering matches with wildcards, so its sites
    need a manual `rg` pass.
  - Each also restores its `comptime` member, its `write_to` and
    `_from_str` arms, and the `is_float8`/`is_half_float`/`is_numeric`
    comparisons the bundled `struct DType` omits
    (`stdlib/std/builtin/dtype.mojo`'s header lists them).
  - Probe: `` __mlir_attr.`#kgen.dtype.constant<ui128> : !kgen.dtype` `` is
    rejected ("DType ui128 is not supported yet",
    `assets/type_error/mlir_kgen_dtype_unknown_mnemonic.mojo`), where the
    pin's `pop.dtype.to_ui8` of it prints `142`.
  - Probe: `pop.dtype.from_ui8` of a code no `Dtype` has (`200`) fails on
    the VM ("DType code 200 is not supported yet"), whose `Value::Dtype`
    holds only a `Dtype`, where the pin and Pliron pass the byte through and
    `pop.dtype.to_ui8` gives `200` back.
  - Restore `_uint_type_of_width`'s `_uint1`, `_uint2`, `_uint4`,
    `uint128`, and `uint256` arms in `stdlib/std/builtin/dtype.mojo`, and
    drop its second `comptime assert`.
  - Restore `SIMD.__hash__`'s `and Self.dtype != DType.float8_e8m0fnu`
    conjunct and its comment in `stdlib/std/simd.mojo`.
  - Probe: `SIMD[DType.bool, 4](fill=True).reduce_bit_count()` prints `4`
    at the pin but fails `pack_bits`'s assert, as do bool widths 2, 128,
    and 256 (found while landing R1586).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R459 A reached struct instance instantiates every constructor
  overload, called or not**

  Problem: `S[5](3)` calls only `__init__(out self, y: Int)`, yet the
  sibling `__init__(out self)`, whose `[1, 2][Self.n]` is out of range at
  `n = 5`, fails the program with "keeps the parameter constant"; the pin
  instantiates only the called overload and prints `8`.
  - `Specializer`'s struct-instance declaration (`mono/specializer.rs`)
    enqueues every `__init__` overload the instance does not disprove, beside
    the copy, move, and destroy members lowering composes by name.
  - A constructor should be instantiated only by a call that reaches it, as
    an ordinary method already is.
  - Pinned by `conformance/probes/constructor_overload_uncalled_instance.mojo`.
  - Found while landing R403 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R71 A bracket application of a generic `def` is not a function
  value**

  Problem: `var g = h[4]; g(1)` over `def h[k: Int](t: Int)` reports "type
  'def[k: Int](Int) thin -> None' cannot be indexed here", where the pin
  binds the instance and prints `5`.
  - Called at once (`h[4](1)`), the application runs. Bound or passed, the
    bracket reaches `Checker::infer_index` (`checker/indexing.rs`) as a
    run-time subscript of the function value.
  - Upstream's bracket on a generic function's name is a parameter
    application whose result is the instance, a thin function value.
  - Probe: `var f = inner[1]; f(x)` over a nested `def inner[m: Int]`
    fails the same way, where the pin prints `3` for `x = 2`.
  - A bare reference to a generic `def` (`apply(as_int, 3)`, `var g =
    as_int`) is rejected at the pin too, so it is not part of this entry.
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R76 `reflect[T]` of a non-struct type is rejected**

  Problem: the pinned Mojo answers `reflect[Int].field_count()` with 1, and
  Mojito rejects it with "requires a struct type".
  - `ReflectQuery::answer` (`param_expr.rs`) raises it for every query but
    `is_struct()` when the check passes no field table (`fields: None`),
    which it does for every type outside the program's struct table, the
    builtin scalars among them.
  - Probe: `show[R.field["x"].T]()` over `def show[T: AnyType]() -> Int:
    return reflect[T].field_count()` and an `Int` field fails the instance
    the same way, where the pin prints `1`.
  - `conformance/probes/template_fallback_reflection.mojo` records the
    observation, made 2026-09-20.
  - Which handle methods answer for a scalar, and with what, needs a probe per
    method before the lever is chosen.
  - Probe: `reflect[Int].field_ref[0](3)` is rejected as "reflect[Int].field_ref
    over a type that is not a struct"
    (`Checker::reflected_fields`, `checker/reflection.rs`), where the pin
    accepts it: the result is `Int`'s `!kgen.scalar<index>` storage, which
    `print` then rejects as not `Writable`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R513 A `comptime for` over another struct's compile-time list is
  rejected**

  Problem: `comptime for v in K.L`, where `struct K` declares `comptime L =
  [1, 2, 3]`, sums the list at the pin, while Mojito reports "Undefined
  variable 'K'".
  - It fails the same way in a plain function and in a generic struct's
    method, so it is the loop header's reading of a type's member, not a
    template gap.
  - Probe: `comptime for v in Self.L` in a method of `struct K` over its
    own `comptime L = [1, 2, 3]` reports "'Self.L' is not a type parameter
    of the enclosing struct", where the pin prints `6`. A scalar member
    reads (`assets/ok/associated_value_self_read.mojo`).
  - Probe: `conformance/probes/comptime_for_other_struct_list.mojo`.
  - Found while landing R8 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R1147 A type proposition over a type parameter is not a runtime
  value**

  Problem: `return conforms_to(T, Copyable)` in `def f[T: AnyType]() ->
  Bool` reports "Undefined variable 'conforms_to'", and `return
  IsSmallCopy[T]` over a predicate alias reports "the Bool-valued comptime
  alias 'IsSmallCopy' read as a value over a type parameter", where the pin
  prints each instance's answer.
  - A closed application (`IsSmallCopy[Int]`) and one over value
    parameters (`positive[k]`) already read as values.
  - `Checker::constraint_proposition` decides a `Conforms` or `Trivial` leaf
    over an unbound type parameter as an unknown hole instead of building
    `ParamContext::conforms` over the parameter's type shape, which the
    elaborator answers per instance.
  - A bare `conforms_to(...)` call has no value inference arm at all, so
    even a closed `print(conforms_to(Int, Copyable))` reports "Undefined
    variable 'conforms_to'" where the pin prints `True`.
  - Found while landing R801 (2026-10-19).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R271 A `field_types()` result in a runtime position is rejected**

  Problem: `var t = reflect[Point].field_types()` compiles at the pin, while
  Mojito reports "'reflect[Point].field_types()' is a compile-time list;
  bind it with 'comptime' and index it".
  - The check's `reflection_value_ty` (`checker/reflection.rs`) types a
    runtime query's answer as an `Int` or a `Bool` alone, and a list of
    types has no runtime value.
  - Upstream's result is a `TypeList` value, which Mojito's checker types
    only in compile-time positions.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R83 A bare reference to a parametric `def` reports a closure
  escape**

  Problem: `var g = inner`, for a nested `def inner[U: Copyable]`, reports
  "closures cannot escape their defining scope (downward funargs only)".
  The pin rejects it too, so the verdict is right and only the message is
  wrong: the pin says "cannot use parametric function as a runtime
  closure".
  - A module-level parametric `def` bound the same way (`var g = as_int`
    over `def as_int[T: Intable](x: T)`) reports the same escape message.
  - The pin rejects `apply(as_int, 3)` with an `invalid call to 'apply'`
    naming the conversion. Mojito's "cannot be converted from the generic
    function …; specialize it explicitly" already says as much.
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

- [ ] **R485 An `Int`-annotated local `comptime` accepts a `Bool` value**

  Problem: `comptime m: Int = 1 > 0` in a function prints `True`, where the
  pin rejects it with "cannot implicitly convert 'Bool' value to 'Int'".
  - The checker's `StmtKind::Comptime` arm (`checker/statements.rs`) drops
    an `Int` annotation before typing the value, so the binding keeps the
    value's own type.
  - A template body's binding over its binders checks its annotation
    (`bind_template_comptime`) and is rejected as at the pin.
  - Probe: `conformance/probes/comptime_int_annotation_bool_value.mojo`.
  - Probe: `comptime y: Int = vs` in `def go[*vs: Int]` prints `(1, 2)`
    for `go[1, 2]()`, where the pin rejects it with "cannot implicitly
    convert 'ParameterList[vs.values]' value to 'Int'". A `TypeList`
    binding's annotation is checked (`check_type_list_annotation`); a
    value pack's is not.
  - Found while landing R312 (2026-10-07).
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

- [ ] **R898 `materialize[it]()` of a compile-time iterator that borrows
  its collection is refused**

  Problem: `comptime it = l.__iter__()` over a local `comptime l = [1, 2,
  3]`, then `for x in materialize[it]():`, fails concrete verification with
  "parameter value constant `…(store_to_mem(…), 0)` is not a closed vector,
  struct, or tuple", where the pin prints `1`, `2`, `3`.
  - The frozen iterator holds a `CtValue::StoreToMem`, which
    `CtValue::is_closed_leaf` (`crates/mojito-types/src/ct.rs`) does not
    accept, and `crossing::ct_to_vm` refuses.
  - Its run-time form is a reference to fresh storage holding the
    referent: the VM's `thaw` already builds one, and Pliron's
    `store_parameter_value` (`lower/consts.rs`) has no arm for it.
  - Found while landing R793 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R797 `comptime for … else` does not parse**

  Problem: an `else:` after a `comptime for` body is "Unexpected token
  Else", where the pin runs it once the loop ends without a `break`.
  - Upstream's `ParamForOp` has an `else` region, reached when
    `paramfor_has_next` is false.
  - Tuple targets (`comptime for a, b in …`) are R1228.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1228 A `comptime for` target does not destructure each element**

  Problem: `comptime for a, b in [(1, 2)]:` is a parse error ("a
  destructuring 'comptime for' target is not supported"), where the pin
  prints `1 2`.
  - Upstream's `parseComptimeFor` stores each element through
    `emitDestructuringPValue(nextValue, targetExpr)`
    (`ParserStmts.cpp:3020`); `StmtKind::ComptimeFor` binds one name, so
    the parser rejects any other target `parse_loop_target` reads.
  - Lever: `ComptimeFor` takes the loop target, and each unrolled step
    binds its leaves to the element's compile-time pieces.
  - Probe: `conformance/probes/comptime_for_destructuring_target.mojo`.
  - Found while landing R1138 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1956 A `SIMDLength` binder read as `Int(n)` in a result type is
  never bound by the elaborator**

  `def g[n: SIMDLength]() -> Tag[Int(n)]` called as `g[2]()` fails
  elaboration ("unsupported monomorphizing `g` return: unresolved value
  parameter `n`"), where the pin runs it.
  - The binder read as `Int(n)` in an argument type alone runs
    (`g[2](Tag[2]())` over `x: Tag[Int(n)]`), as does an `Int` binder in
    a result (`-> Tag[n + 1]`).
  - With both (`def g[n: SIMDLength](x: Tag[Int(n)]) -> Tag[Int(n)]`),
    `g[2](Tag[2]())` fails as "unsupported monomorphizing `g`: unresolved
    value parameter `n`", and so does `g[m](…)` over `comptime m =
    SIMDLength(2)`.
  - The image is `types::simd_length_image`, `Int(mlir_value=)` of the
    binder's `_mlir_value`, so the result's substitution misses the
    explicit binding: find where mono closes a signature's result type.
  - Probes: the three shapes above, with `Tag[n: Int]` a field-less
    struct.
  - Found while landing R1835 (2026-11-06), whose plan recorded the
    argument-and-result shape failing before it.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1847 A compile-time construction whose initializer overload
  fails reports a generic error, not the pin's overload failure**

  `Holder[Key("x")]()` over `@fieldwise_init struct Key` (`var v: Int`)
  and `struct Holder[k: Key]` fails with "not a compile-time Int
  constant: not an associated comptime expression", where the pin reports
  "no matching function in initialization" at `Key("x")`.
  - Cause: `Checker::selected_application`
    (`checker/comptime_validation.rs`) answers `Option`, so a call in a
    parameter position whose overload set has no viable candidate is
    `None`, and `eval_associated_ct`'s fallback (`checker/constraints.rs`)
    reports the generic error instead of the overload resolution's.
  - Lever: report the run-time overload resolution's rejection when no
    candidate applies, as a body's call of the same initializer does.
  - Probe: `assets/type_error/shell_default_alias_bad_construction.mojo`
    (the same construction in a module alias a struct parameter's default
    names) can then pin the wording with `# expect:`.
  - Found while landing R1788 (2026-11-02).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2000 A construction through a module type alias in a module
  `comptime`'s parameter argument is rejected**

  `comptime Zero = Keyed[V(5, 6)]` over `comptime V = SIMD[DType.int32,
  2]` and `struct Keyed[key: V]` fails with "not a compile-time Int
  constant: not an associated comptime expression", where the pin binds
  the vector.
  - The same `Keyed[V(5, 6)]()` written in `main` runs, as do
    `comptime z = V(5, 6)` and `Keyed[SIMD[DType.int32, 2](5, 6)]` in the
    module alias, so only the alias-named callee in a module alias's
    application argument misses.
  - A user struct's alias fails the same way: `comptime Zero = Keyed[P(5)]`
    over `comptime P = Pt`.
  - Look first at `Checker::selected_application_at`
    (`checker/comptime_validation.rs`), whose `None` leads
    `eval_associated_ct` to report the generic error (R1847).
  - Probe: `assets/ok/module_type_alias_reads.mojo` runs (it fails in
    `run`, `vm_ok`, `verify`, and `roundtrip` today).
  - Found while landing R1996 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1846 A `SIMD[…]` type application before `SIMD`'s shell
  registers reads its brackets positionally**

  An alias or struct shell linked above `struct SIMD` (`comptime U256 =
  SIMD[DType.uint64, 4]`, a shell parameter typed `key: U256`) resolves
  `SIMD[…]` through `Checker::positional_simd_dims`
  (`checker/type_resolution.rs`), not the declared struct's parameters.
  - The pin resolves the declaration on demand, so the application binds
    `SIMD`'s declared parameters wherever it is spelled.
  - `simd_declared_application` keeps the registration test for this case
    alone, beside R1790's `_` width.
  - Lever: let such an alias or shell wait for `SIMD`'s shell
    (`shell_awaits_declarations` and the shell-loop alias retry in
    `check_program`), then drop the registration test.
  - Found while landing R1788 (2026-11-02).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1782 A float literal narrowed past its format's range is `inf`,
  where the pin's `float_literal_convert` gives `nan`**

  Problem: `var a: Float32 = 3.5e38` prints `inf`, where the pin prints
  `nan`.
  - Probe: at the pin, `Float32(-3.5e38)` and
    `Scalar[DType.float16](70000.0)` are `nan`, while `var b: Float64 =
    -1e400` is `-inf`, `bfloat16` of `1e39` is `inf`, and `Float32` of
    `1e-50` is `0.0`. Mojito prints `inf`, `-inf`, `inf`, and `0.0` for the
    first four.
  - `return 3.5e38` from a `Scalar[dtype]` function at `float32` does the
    same.
  - Lever: the rule belongs in `CtLane::of_float_literal` (`ct.rs`), which
    MIR's `MaterializeLiteral` and the literal coercions share. Probe more
    values at the pin before stating it.
  - Found while landing R1507 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1783 A compile-time float literal cannot be `nan` or infinite**

  Problem: upstream's `FloatLiteral.nan`, `infinity`, `negative_infinity`,
  and `negative_zero` are not ported, because their attributes
  (`#pop.float_literal<nan>`, `<inf>`, `<neg_inf>`, `<neg_zero>`) have no
  `CtValue::PopFloatLiteral`.
  - Cause: `mojito_common::literal::FloatLiteral` is an exact rational
    with a negative-zero flag, with no special values.
  - Probe: `print(FloatLiteral.nan.is_nan(),
    FloatLiteral.infinity._is_normal())` prints `True False` at the pin.
  - The `float_literal_isa` folds (`fold::fold_literal_attribute`) answer
    `nan` as always false until then.
  - Probe: `print(1 / 0)` prints `nan` at the pin; Mojito types it by the
    unfolded `#pop<float_literal_bin<truediv 1.0, 0.0>>`, which elaboration
    rejects ("keeps symbolic type").
  - Found while landing R1507 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1963 A value parameter typed `IntLiteral[_]` stays its binder
  after monomorphization**

  `def g[x: IntLiteral[_]](): print(x)` with `g[3]()` fails elaboration
  with "keeps the parameter constant `x`", where the pin prints `3`.
  - The instance `g$mono$V3$V3` binds both `x` and its hidden
    `$value_x`, yet a run-time read of `x` (a `const` of the parameter)
    is not substituted, while `[x: Int]` is.
  - Look at `native::mono`'s substitution of a `Const::Param` whose
    binder's type is a literal over another binder of the same signature.
  - Probe: `def g[x: IntLiteral[_]](): comptime y = x * 2 + 1;
    print(type_of(y)(), y)` with `g[3]()` prints `7 7` at the pin; today
    `type_of(y)()` alone prints `7`, and reading `y` fails the same way.
  - Found while landing R1849 (2026-11-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1968 A numeric literal's value is not an instance of the declared
  `IntLiteral` struct, so its construction and copy are checker rules**

  `IntLiteral[v]()`, `Self()` in a literal's member, and
  `IntLiteral[v](copy=x)` are built by the checker's
  `literal_construction` and `infer_instance_construction`
  (`call_inference.rs`), where upstream runs the declared
  `__init__(out self)` and the synthesized copy.
  - Cause: the elaborator declares no struct instance for a literal type,
    so `MirDeclarations::struct_instance` answers `None` for one, and a
    `struct.create` of it would not verify.
  - Stand-ins to delete: those two checker arms, MIR's
    `materialized_literal_construction` (`mir/lower_expr/calls.rs`), and
    the numeric-literal guard in `struct_instance` (`mir.rs`).
  - `builtin_copy_is_value_read` (`types.rs`) admits a numeric literal
    for the same reason.
  - Lever: declare `IntLiteral[v]` and `FloatLiteral[v]` as zero-field
    instances, so `struct.create` of one is the VM's empty literal struct.
  - A use's `MaterializeLiteral` then converts the call's result register,
    since it reads the number off the register's type (R1850).
  - Probe: `assets/ok/int_literal_struct.mojo`,
    `int_literal_value_type.mojo`, and `simd_literal_convert_attribute.mojo`
    print unchanged on both backends.
  - Found while landing R1851 (2026-11-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1969 A numeric literal's operators and conformances are compiler
  rules, where upstream's are `IntLiteral`'s declared dunders and
  conformance list**

  `TyKind::as_non_simd_struct` and the elaborator's
  `declared_operator_receiver` (`mono/specializer.rs`) exclude a numeric
  literal application, so no declared-struct rule reaches it.
  - The checker's literal rule (`Checker::literal_rule_type`,
    `operators.rs`) types `/`, unary `-`/`~`, and a mixed
    `IntLiteral`/`FloatLiteral` pair instead of the declared dunders.
  - `conformance::modelled_conformance` answers a literal's traits by
    name: it calls a literal `Comparable` and `Equatable`, which the
    declared structs do not list.
  - `scalar_declares_math_trait` (`method_calls/resolution.rs`) answers
    `Ceilable`/`Floorable`/`Truncable` by name too.
  - Done when those exclusions and arms are gone and a literal's
    operators resolve through `infer_literal_operator` alone.
  - Probe: `print(2 ** 10, -(3), ~5, 1 / 2)` keeps printing
    `1024 -3 -6 0.5`, through the declared dunders.
  - Found while landing R1851 (2026-11-06).
  - Depends on R1968, and R1855 for the declared `__pow__`.
  - Model: Fable, Not Planned.

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

- [ ] **R272 A reflected field-name list is an `Array` of `String`, not
  of `StaticString`**

  `materialize[names]()` over `comptime names = reflect[T].field_names()`
  is an `Array[String, 2]` in Mojito and an `Array[StaticString, 2]` at
  the pin, so the crossing diagnostic spells `Array[String, Int(2)]` where
  the pin spells `Array[StringSpan[ImmStaticOrigin], Int(2)]`.
  - Since R1657 an element read (`names[i]`, `field_names()[i]`) is a
    `StaticString`, the `_kgen` application over the element's
    `!kgen.string`.
  - The list is typed `ParamList(value(!kgen.string))` as upstream's
    `#kgen.struct_field_names` is (`ReflectQuery::meta`), but still holds
    `CtValue::Str` literal values (`ReflectQuery::answer`), where
    upstream's elements are `!kgen.string`.
  - Probe: an element read in a generic body takes the `value` field of
    a `!kgen.string`-typed node (`ParamContext::field` in the
    `FieldNames` index arm of `reflection.rs`), which the closed list's
    `StringLiteral` element answers once the elaborator binds `T`
    (`assets/ok/reflection_field_name_materialize.mojo`); upstream reads
    the element itself (found while landing R1861, 2026-11-06).
  - The whole list's type is `reflected_names_array_ty`'s `Array[String,
    N]`, and `reject_bound_list_crossing` spells it (`reflection.rs`).
  - Probe: `var a = materialize[names]()` then
    `_unqualified_type_name[type_of(a)]()` prints `Array[String, …]`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R497 A reflected field-type list materialized whole is rejected**

  Problem: `var all = materialize[types]()` over `comptime types =
  reflect[T].field_types()` prints the field count through `len(all)` at
  the pin and is rejected in Mojito with "type-valued or symbolic comptime
  values cannot materialize at runtime".
  - The pin's `field_types()` is a zero-sized `TypeList`, so the runtime
    value carries only its length.
  - A closed subject is rejected too, so this is no template-form gap.
  - Probe: `conformance/probes/reflected_field_types_materialize.mojo`.
  - Found while landing R365 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R460 An out-of-range display index over a binder fails as an
  unsupported constant**

  Problem: `comptime q = [1, 2][n]` in `def f[n: Int]()` called as `f[5]()`
  stops with "keeps the parameter constant `[1, 2][n]` in elaborated MIR",
  where the pin rejects the instantiation at compile time.
  - The verdict is right; the words are internal.
  - The same cause as R327: `Specializer::param_constant`
    (`mono/specializer.rs`) drops the out-of-range error, so the verifier
    reports the surviving constant instead of a failed instantiation.
  - Found while landing R403 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R499 An invalid SIMD width computed per instance is reported as
  unverifiable MIR**

  Problem: `comptime w = twice(i + 1)` then `SIMD[DType.int32, w](1)` in a
  served `comptime for` stops with "specialized MIR that does not verify:
  ... SIMD width 6 is not a positive power of two" when a copy computes 6.
  - The pin reports "SIMD vector length must be a power of two between 1
    and 2^15" at the use.
  - The same width also read under a `comptime if` over `comptime b =
    flag(i)` reports "keeps symbolic type `SIMD[DType.int32, twice(i + 1)]`
    in elaborated MIR" instead.
  - A clone used to report "SIMD width must be a positive power of two, got
    6".
  - Both compilers reject the program; only the words differ.
  - Found while landing R253 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R345 A list computed from packs still open answers only its length
  and its elements**

  Problem: `Self.Ts.reverse().all_conforms_to[Copyable]()` in a variadic
  struct's method stops with "TypeList.all_conforms_to over the computed
  list … whose packs are still parameters".
  - `all`, `any`, and `contains` over such a list are rejected the same way.
  - Probe: `comptime if TypeList.of[T, U]().contains[T]():` in `def
    same[T: AnyType, U: AnyType]` (with `Trait=AnyType`) stops with
    "TypeList.contains over the computed list ... whose packs are still
    parameters", where the pin prints `True`: the alias over a template's
    own parameters is such a list.
  - Over a closed list (`TypeList.of[…]().reverse()`) each of them folds.
  - Upstream's `slice`, `splat`, `map`, `filter_idx`, and `reduce` have no
    spelling in Mojito at all.
  - The lever: the checker builds the query over the computed list as its
    `ParamKind::PackQuery` operand, which the elaborator already decides
    (`proposition_constraint`'s `Expr` operand).
  - Depends on R1951, whose pack operand this generalizes to a computed
    list.
  - Model: Fable, Planned.

- [ ] **R348 A spread of a computed list binds only `Tuple`**

  Problem: `Holder[*Self.Ts.reverse()]()` over a user variadic struct stops
  with "call spread outside a specialized type pack", where the pin prints
  `3` for the flipped holder's length.
  - `Tuple[*Self.Ts.reverse()]` and
    `Tuple[*TypeList._concat[A.values, B.values]()]` resolve.
  - A `Variant[*Ts.reverse()]` is unspelled the same way.
  - Probe: a `Holder[*Ts: Movable]` whose method returns
    `Holder[*Self.Ts.reverse()]()`.
  - Probe: a computed list read at run time, `comptime r = Ts.reverse()`
    then `var x = r` (or `comptime c = TypeList._concat[Ts.values,
    Ts.values]()`), stops with "a computed TypeList read at run time",
    where the pin prints `3` and `6` for `len(x)` over `Int, Bool,
    String`. The read is `TypeList[Trait=Movable, <the computed list>]()`,
    which binds the computed list as `TypeList`'s `values`
    (`Checker::infer_type_list_binding_read`).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R415 A static call spreading a variadic struct's collector in its
  own method is rejected**

  Problem: inside `struct V[*Ts: Writable]`, a method that forwards its
  `*b: *Self.Ts` collector to a static method (`Self.count(*b)`,
  `V[*Self.Ts].count(*b)`) fails with "no symbolic rule for an unbound
  pack", where the pin runs it.
  - The instance-method form (`self.each(*b)`) runs.
  - A bare `Self.` static on a parametric struct infers the struct's
    parameters from the arguments (`infer_struct_static_method`) instead of
    binding the enclosing `Self`, and a forwarded pack infers nothing.
  - Found while landing R256 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1071 Spreading a homogeneous collector into another call is
  rejected**

  Problem: `def outer(*xs: Int): return inner(*xs)` stops in the checker
  with "call spread outside a specialized type pack", where the pin passes
  the collector on and prints `3` for `outer(1, 2, 3)`.
  - `Checker::forwarded_pack_operand` (`checker/comptime_validation.rs`)
    takes a spread only of a type pack (`*args: *Ts`). A homogeneous
    collector's spread falls to `infer`'s `ExprKind::Spread` arm
    (`checker/inference.rs`), which rejects it.
  - Upstream forwards the caller's `VariadicList` whole. Since R1103 a
    type-pack spread passes the caller's pack itself, and
    `verify_pack_spread` already accepts a `VariadicList`-typed spread
    operand, so only the checker's binding is missing.
  - Probe: the program above (pin: `3`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1303 A `mut *xs` collector is a parse error**

  Problem: `def mutate(mut *xs: Int)` stops with "Expected a parameter
  name after the convention", where the pin types the collector
  `VariadicList[elt_is_mutable=True, ..., Int, False]` and `for ref x in
  xs: x += 1` mutates the caller's values (`mutate(a, b)` prints `2 3`).
  - `Parser::parse_param_list` (`crates/mojito-parser/src/parser/items.rs`)
    takes `*` after a convention only for `var`. `mut`, `read`, and `ref`
    go through `parse_convention_and_name`, which wants a name.
    `docs/grammar.md` already spells `[convention] '*' NAME`.
  - Past the parser, the call builds a `read` collector's element pointers
    over the caller's places (`borrowed_element`,
    `mir/call_defaults.rs`). A `mut` collector needs the same pointers as
    mutable loans, and its `VariadicList` instance at `elt_is_mutable=True`.
  - Probe: the program above (pin: `2 3`).
  - Probe: a heterogeneous `mut *args: *Ts` parses the same way. Past the
    parser, `build_variadic_pack` (`mir/variadics.rs`) already builds a
    mutable reference pack for a `mut` collector, which nothing has run.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R436 A struct's `comptime` value member read through the type name
  is rejected**

  Problem: `print(E.k)` over `struct E: comptime k = 3` prints `3` at the
  pin, while Mojito reports "Undefined variable 'E'", and `G(E.k)` reports
  that no constructor overload matches.
  - The same member read as `Self.k` inside the struct runs
    (`assets/ok/associated_value_self_read.mojo`).
  - Probe: a run-time construction's bracket list reading a member, `var t
    = TypeList[Trait=AnyType, E.types]()` over a raw-list member, fails
    with "Undefined variable 'E'", and `TypeList[Trait=AnyType,
    TypeList.of[Trait=AnyType, Int].values]()` and `TypeList[Trait=AnyType,
    S[Int, Bool]._types]()` fail the same way, where the pin prints `2 1`;
    in a type annotation or a `comptime` position each runs.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R438 A `ref` binding of a pack element inside a `comptime for` fails
  elaboration**

  Problem: `comptime for i in range(Ts.length): if i == idx: ref arg =
  args[i]; arg.write_to(w)` runs at the pin, while Mojito stops with "slot
  `arg` typed over a comptime for index is used outside its loop".
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R666 An associated type's value parameter is not elaborated**

  The elaborator answers `C.Member[o]` for a parameterized associated type
  whose explicit parameters are types or origins, but rejects one with a
  value parameter (`comptime Buf[n: Int] = InlineArray[Self.T, n]`) as
  unsupported.
  - The one member lookup both phases share,
    `StructDeclaration::associated_member`, has no binder to pair with
    `AssociatedParameter::Value`, which names its parameter by spelling,
    so it answers `AssociatedLookupError::ValueParameter`.
  - The checker binds it by spelling too (`resolve_parameterized_member`).
  - The elaborator's `declared_associated_type` rejects it from that
    error, and `inline-parametric`'s `StructDeclaration::associated_type`
    leaves such a projection open.
  - No executing fixture reaches it.
  - Done when the member's value parameter keys by a binder identity and
    the elaborator binds it from the application's `TyArg::Val`.
  - Found while landing R661 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R895 A type indexed by a `comptime for` variable is spelled with
  mangled names in diagnostics**

  A diagnostic names an element type over the loop variable as
  `Ts[__module$std$builtin$$stubs$paramfor_next_value[__module$std$range$_ZeroStartingRange[DType.int]]($iter)]`,
  where the pin writes
  `Ts.values[SIMDLength(paramfor_next_value[_ZeroStartingRange](iter))]`.
  - Both spell the stub application, since the variable is an alias of
    `paramfor_next_value` over the iterator.
  - The gap is the linked module prefixes, the dtype argument, and the
    binder's `$`, all from the `Display` of `ParamExpr`
    (`crates/mojito-types/src/param_expr.rs`).
  - Pinned by `assets/type_error/pack_element_missing_bound.mojo` and its
    row in `conformance/assets-mojo-errors.tsv`.
  - Found while landing R791 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R899 A reference to a compile-time value is not typed
  `ComptimeOrigin`**

  Problem: a run-time read of `comptime it = l.__iter__()` is rejected with
  "cannot materialize comptime value of type '_ArrayIter[…, 3 : …]' to
  runtime because it is not 'ImplicitlyCopyable'", where the pin names the
  type `_ArrayIter[Int, Int(3), ComptimeOrigin]` and says "the type contains
  an origin referring to a compile-time value".
  - Upstream types the borrow of a parameter-domain value
    `ref[ComptimeOrigin] T` (`PMBValue`/`PMRValue`,
    `lib/MojoParser/IRValues.cpp`).
  - Mojito's iterator keeps the binding's origin, and `bind_comptime_memory`
    (`crates/mojito-mir/src/mir/calls.rs`) types the `StoreToMem` handle
    `Origin::Static`.
  - The fix is an `Origin::Comptime { mutable }` variant the checker gives a
    borrow of a compile-time value, and the pin's diagnostic for a type
    that holds one.
  - Probe: `comptime s = Span(L)` over `comptime L = [1, 2, 3]`, then a
    run-time `len(s)`, is rejected at the pin ("cannot materialize
    compile-time value of type 'Span[Int, ComptimeOrigin]' to a runtime
    value"), and `comptime n = len(s)` prints `3`. Both wait on R1728
    first, since `L` is a raw element list rather than an `Array`.
  - A `ref` parameter's compile-time argument (`ParamOp::StoreToMem`,
    `ParamContext::store_to_mem`) is typed `Origin::Static` too, and so is
    the origin binder it binds (`f[Span(l)]()` over a `List[Int]` binder):
    move it with `bind_comptime_memory`.
  - Probe: `comptime b = c[:2]` over `comptime c: List[Int] = [1, 2,
    3]`, then a run-time `print(b[1])`, prints `2` for a module `c` and
    stops in the VM with "use after Pointer deallocation" for a local one.
    Mojo 1.3.0.dev2026102605 rejects both with "use of a never-initialized
    interior reference 'ComptimeOrigin["element"]'", and `comptime n =
    len(b)` prints `2`.
  - Found while landing R793 (2026-10-14).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1301 A struct alias applying a keyword argument to an infer-only
  parameter does not resolve**

  Problem: the bundled `_VariadicListIter` spelling upstream's `comptime
  variadic_list_type = VariadicList[origin=Self.elt_origin, Self.elt_type,
  Self.is_owned]` makes the library fail to check ("type
  '_VariadicListIter[elt_type, is_owned]' has no associated type
  'variadic_list_type'"), so it names no `origin`.
  - The keyword names `VariadicList`'s infer-only `origin`, which the
    associated-alias resolution does not bind.
  - Found while landing R743 (2026-10-23).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1639 A struct's type member read through an application over a
  symbolic parameter has no receiver**

  `T[t].current.answer()` inside `def f[t: Int]()`, over `struct T[n:
  Int]` with `comptime current = Self`, stops with "type 'T[t]' has no
  field 'current'", where the pin prints `3` for `f[3]()`.
  - The checker reads a type member through a struct as the source
    spelling of the type it resolves to (`type_member_spelling`,
    `checker/initializer_list.rs`). `ct::source_type` has no spelling for
    a type whose argument is still a parameter expression (`T[t]`).
  - MIR lowering recognizes a static call's type receiver only by its
    syntax (`type_receiver_name`, `mir/lower_expr/expr_method.rs`): a
    `TypeApply`, `type_of(…)`, or a bare name. A `Member` receiver the
    checker resolved to a type is lowered as a value.
  - Upstream reads the member as its value, a type the elaborator
    substitutes per instance. A checked fact that a receiver denotes a
    type, which lowering reads in place of its syntax, closes both.
  - Probe: `T[t].boxed.n` and `T[t].boxed(10).get()` over `comptime boxed
    = Box[Self.n + 1]`, which the pin prints as `4` and `14` for `t = 3`.
  - Probe: a static call through a generic alias's application over a
    symbolic argument, `V[k].get()` inside `def f[k: Int]()` over
    `comptime V[k: Int] = W[k]`, fails with "Undefined variable 'V'",
    where the pin prints `3` for `f[3]()`: `alias_application_spelling`
    (`checker/initializer_list.rs`) has no spelling of `W[k]`, while the
    construction `V[k]()` runs.
  - Probe: a member read through a bare struct name whose infer-only
    parameter has no default, `comptime v = T.four()` and `g[T.four()]()`
    over `struct T[n: Int, //]` with `comptime four = T[n=4]` and `def
    g[t: T]()`, fails with "Undefined variable 'T'" and "cannot infer type
    parameter", where the pin prints `1`: `type_member_read` resolves the
    receiver to an instance first, and bare `T` binds no `n`. With a
    default (`n: Int = 2`) both run since R1562.
  - Found while landing R1632 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1680 `reflect[T].name()` spells an origin-parametric type with
  Mojito's parameter list, not upstream's**

  `reflect[StaticString].name()` prints `std.string.StringSpan[{}]`, where
  the pin prints `std.collections.string.string_span.StringSpan[{}, {}]`.
  - `#kgen.get_type_name` spells every parameter of the type, the inferred
    `mut` and the defaulted `address_space` included, where
    `TypeNameSpeller` (`mojito-symbol`) sees only the arguments a
    `TyKind::Struct` carries, and Mojito's `Span` declares no
    `address_space`.
  - The pin's second `{}` beside an origin is not located. A variadic type
    pack already spells as the pin's `<unprintable>, {}` (`Tuple`,
    `Variant`).
  - Probe: `_unqualified_type_name[StaticString]()` prints
    `StringSpan[ImmStaticOrigin]`, where the pin prints `StringSpan[{}, {}]`
    (`unqualified_instance_name`, `mojito-symbol`).
  - Probe: `reflect[T].name()` in `def f[T: AnyType](x: T)` over
    `f(Span(xs))` prints `std.span.Span[SIMD[DType.int, 1], {}]`, where the
    pin prints `std.collections.span.Span[True, {}, SIMD[DType.int, 1], {},
    0]`.
  - Probe: the same `f(xs.unsafe_ptr())` prints `Pointer[Int,
    ImmStaticOrigin]`, where the pin prints
    `std.memory.pointer.Pointer[True, {}, SIMD[DType.int, 1], {}, 0]`:
    `Pointer` is the intrinsic `TyKind::Pointer`, which the speller leaves
    to its `Display`.
  - Probe: `_unqualified_type_name[type_of(buffer)]()` of `print`'s
    `_FlushingWriteBuffer` over a `RawFileDescriptor` prints
    `_FlushingWriteBuffer[RawFileDescriptor, 4096 : SIMD[DType.int, 1]]`,
    where the pin prints `_FlushingWriteBuffer[{}, {},
    std.io.file_descriptor.RawFileDescriptor, 4096 : SIMD[DType.int, 1]]`:
    the infer-only origin and writer parameters are omitted.
  - The module paths in these probes are R1679's.
  - Found while landing R1669 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1718 A failed `where` clause's diagnostic spells the compiled
  constraint, where the pin spells the clause as written**

  `lane_kind(Int32(1))` over `where dtype.is_floating_point()` reports
  "expected 'not [0] : SIMD[DType.uint8, 1] == dtype._as_ui8() & [64] :
  SIMD[DType.uint8, 1] == true'", where the pin's note says "expected
  'dtype.is_floating_point()'".
  - `ConstraintFailure::reason` (`checker/generics.rs`) displays the
    `GenericConstraint`, which keeps no source spelling: a `Bool` leaf
    shows its folded parameter expression compared with `true`.
  - Upstream's note quotes the clause's source range.
  - Probe: the "lacking evidence" reason of
    `assets/type_error/where_dtype_predicate_lacks_evidence.mojo` spells
    the same folded form, where the pin's note says "needs evidence for
    'dtype.is_floating_point()'".
  - Found while landing R1661 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1736 An instance is keyed by its parameter applications'
  values, so a default whose call fails is evaluated where the pin never
  reads it**

  The elaborator answers every kept parameter's application when it keys
  an instance, where the pin keys it by the unevaluated application
  (`S[h(3)]` is not `S[6]`) and evaluates it only where a surviving read
  materializes it.
  - `substitute_arg` (`mono/substitute.rs`) answers a struct type
    argument's application to key the struct instance, and a function
    instance answers a parameter its body reads at keying, even when that
    read is dead once an `@always_inline` callee is inlined.
  - A 2026-10-30 prototype that kept a closed residual in struct
    arguments, and mangled it, broke `Array`'s `length` in
    `_write_int`, receiver binding ("solution for `_x` is not
    constant"), and parameter-constant reads: every consumer of a
    struct's bindings must demand the residual.
  - Probe: `var x = F[DType.int32]()` and `x.g()` over `struct F[d: DType,
    *, _x: NoneType = _c[d]()]` whose `_c` asserts a float dtype report
    "function instantiation of `_c` failed", where the pin runs both.
  - Probe: `comptime x = F[DType.int32].g()` over the same `F` fails the
    same way.
  - Probe: the same over an `Int`-typed default (`_x: Int = _c[d]()`)
    fails the same way.
  - Probe: `var s = String(); _k.write_to(s)` in `def f[d: DType, *, _k:
    NoneType = _c[d]()]() -> Int` at `DType.int32` fails, where the pin
    prints the result (`print(_k)` and a generic `Writable` call fail at
    both).
  - A static method or `comptime` member reached without constructing,
    and an unread parameter of a function, already run (R1720).
  - Found while landing R1720 (2026-10-30).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1732 A layout query over a generic struct instance the program
  never constructs has no layout**

  `size_of[List[Int]]()` stops with "keeps the parameter constant
  `kgen.get_sizeof[List[Int]](…)` in elaborated MIR", where the pin prints
  `24`.
  - The elaborator's layout oracle (`layout_query_answer`,
    `native::mono`) lays out non-generic structs from
    `StructCatalog::fill_layout` and generic instances only once the
    elaboration has discovered them, so an instance the query alone names
    has no fields.
  - Upstream's `get_sizeof` lays out the instance's fields from the
    struct's declaration at its arguments.
  - The failure is swallowed into the verifier's message, since
    `Specializer::param_constant` keeps a parameter constant whose
    evaluation fails `Unsupported`.
  - Probe: `size_of[G[Int]]()` in `def sz[G: __generator_type[T:
    AnyType] AnyType]()`, called as `sz[Pair]()` over `comptime Pair[T:
    AnyType] = Tuple[Int, Int]`, stops the same way, where the pin prints
    `16`. Over a non-generic struct body it prints its size.
  - Found while landing R1634 (2026-10-30).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1733 A layout query under a target other than the compilation's is
  rejected, where the pin lays out under that target**

  `size_of[Int, CompilationTarget._unknown()]()` stops with "keeps the
  parameter constant `kgen.get_sizeof[Int](#kgen.target<triple = "", …>)`
  in elaborated MIR", where the pin prints `0`.
  - `layout_query_answer` (`native::mono`) answers only a `!kgen.target`
    equal to `NativeTarget::kgen_target`, and reports any other as
    unsupported.
  - Upstream's `get_sizeof` and `get_alignof` lay out under the target
    operand's own data layout and index width.
  - Probe: `align_of[Int, CompilationTarget._unknown()]()` and
    `bit_width_of[Int, CompilationTarget._unknown()]()` stop the same way.
  - Found while landing R1634 (2026-10-30).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1754 A compile-time `List + List` over two `comptime` values is
  rejected**

  `f[A + B]()` over `comptime A: List[StaticString] = ["sm_80", "sm_86"]`,
  `comptime B: List[StaticString] = ["sm_90"]`, and `def
  f[archs: List[StaticString]]()` counting its `comptime for` stops with
  "value of type 'List[StringSpan]' cannot be implicitly copied …
  (argument 1 to method '__add__')", where the pin prints `3`.
  - A `comptime C = A + B` binding fails the same way, and so does a body
    holding the sum that is never called: the checker applies run-time
    copy rules to a compile-time operand.
  - The run-time `g(L)` over `def g(var x: List[Int])` is rejected at the
    pin too ("cannot materialize comptime value"). Only the compile-time
    read is the gap.
  - Probe: `std.sys.info`'s `CompilationTarget._has_blackwell_tcgen05`
    (`_SM_100X_ARCHS + _SM_101X_ARCHS + _SM_103X_ARCHS`), then the
    `_is_sm_100x_or_newer`, `_is_sm_9x_or_newer`, and `_is_sm_8x_or_newer`
    chain and the free `_is_sm_9x_or_newer` and `_is_sm_100x_or_newer`
    over it, which the bundled module omits.
  - Found while landing R1635 (2026-10-31).
  - Depends on R1756 (the Nvidia methods it restores need it).
  - Model: Opus, Not Planned.

- [ ] **R1755 A type applied through a struct-valued parameter's static
  call keeps a symbolic type after elaboration**

  `StringLiteral[t.__triple()]()._is_identical("x86_64-unknown-linux-gnu")`
  in `def g[t: CompilationTarget]() -> Bool`, called as
  `g[CompilationTarget.current()]()`, stops with "register r0 keeps
  symbolic type `StringLiteral[$comptime$g$c0($_mlir_value_t, t)]` in
  elaborated MIR", where the pin prints `True`.
  - The lifted `$c0` thunk reads `t`'s inferred `_mlir_value`, and the
    elaborator does not resolve the application once `t` is closed.
  - `comptime s = t.__triple()` fails the same way.
    `StaticString(t.__triple())` at run time works, and
    `Self.__triple()` inside the struct works.
  - Probe: port `CompilationTarget._eq_triple_and_arch[other]`, which
    `std.sys.info` omits for this reason.
  - Found while landing R1635 (2026-10-31).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1756 The bundled library rejects a lifted `comptime if` that
  reads `Self()` in a struct with an infer-only parameter**

  Splicing upstream's Nvidia section into `std.sys.info` makes
  `comptime if not Self().is_nvidia_gpu():` in
  `CompilationTarget._is_nvidia_gpu_any` lift to a `$c0` thunk that
  "names parameter `_mlir_value` of
  `__module$std$sys$info$CompilationTarget` that no enclosing declaration
  binds", so the bundled library cannot be packaged.
  - The failure shows when the whole library is verified
    (`src/package.rs`), and then on every program, called or not.
  - The planning session saw the same struct run as a user module (`-I`);
    the first step is to find why the lifted thunk keeps the struct's
    parameter unbound only there.
  - Probe: port `_is_nvidia_gpu_any`, the `_is_sm_8x` … `_is_sm_121x`
    methods, `_is_sm_110x_or_newer`, `_is_sm_120x_or_newer`, the free
    `_SM_*X_ARCHS` lists, and the free `_is_sm_9x`, `_is_sm_100x`,
    `_is_sm_110x`, and `_is_sm_120x_or_newer`, then
    `print(CompilationTarget.current()._is_sm_9x())` prints `False`.
  - Restores `SIMD`'s `_is_sm_9x_or_newer` guards with R1754.
  - Found while landing R1635 (2026-10-31).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1757 `__get_current_function_name()` is undefined**

  `var s = __get_current_function_name(); s += "!"; print(s)` in `main`
  stops with "Undefined variable '__get_current_function_name'", where the
  pin prints `main!`.
  - At the pin it is the enclosing function's short name: `m` for a static
    method, `g` for `g[3]`. It is usable in `comptime`.
  - Check with the pin whether its type is `StringLiteral` or `String`:
    `type_of` prints `String` for both it and a literal, so that probe
    does not settle it.
  - Probe: port `std.sys.info.current_accelerator` and its `std.sys`
    re-export. On a CPU target its `comptime assert` fails with upstream's
    "current_accelerator() requires an accelerator compilation target"
    text.
  - Found while landing R1635 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1925 A decorator before a struct's `comptime` member is a parse
  error**

  `@deprecated(use=Ts)` before `comptime element_types = Self.Ts` stops
  with "a decorator in a struct body must precede a 'def' method", where
  the pin accepts it and warns at each use of the member.
  - Probe: the bundled `VariadicPack` declares `element_types` without
    upstream's `@deprecated(use=Ts)`.
  - Lever: the struct-body parser takes decorators before a `comptime`
    member; the warning itself is R1164's.
  - Found while landing R1302 (2026-11-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1979 `SourceLocation`, `source_location`, and `call_location` are
  not bundled**

  `std.reflection`'s source-location API is missing, so the bundled
  `debug_assert` takes no `location=` argument and a failed assertion cannot
  report where it failed.
  - Upstream builds all three on `kgen.source_loc`, whose `inline_count`
    names the call site an `@inline(.always)` chain was entered from.
  - The pin's `debug_assert[assert_mode="safe"](False, "x")` reports
    `At: <file>:<line>:<col>:` at the call's condition, through the
    `@inline(.always)` `debug_assert`.
  - Once bundled, restore `debug_assert`'s `location: Optional[
    SourceLocation] = None` parameters and its `call_location()` calls
    (`stdlib/std/builtin/debug_assert.mojo`'s header note).
  - Probe: `debug_assert(True, location=None)` matches no overload, where
    the pin accepts it.
  - Found while landing R1598 (2026-11-06).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R2015 A local non-owned `VariadicPack` variable fails
  elaboration at its destruction**

  `var c = pack.copy()` over a read `VariadicPack` parameter fails with
  "unsupported dependent type `Ts[...paramfor_next_value...($iter)]` has
  no concrete MIR declaration fact", where the pin runs it.
  - `len(pack.copy())`, a temporary, runs; so the binding's destruction is
    the difference.
  - The pack declares `__del__is_trivial = not Self.is_owned`, and its
    `__deinit__`'s `comptime for` over `Self.Ts` sits under `comptime if
    Self.is_owned`: either the drop ignores the trivial hook here, or the
    elaborator lowers the dead arm over the symbolic pack.
  - Probe: `def total[*Ts: Writable](pack: VariadicPack[origin=_,
    element_trait=Writable, False, *Ts]) -> Int: var c = pack.copy();
    return len(c)`, called through a `*args: *Ts` forward.
  - Found while landing R1832 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2021 A dynamic-value rejection reads a builtin method that
  returns its receiver as a call result**

  `var b = True` then `comptime i = b.__bool__() or False` reports "cannot
  use a dynamic value in comptime initializer", where the pin reports
  "operator argument".
  - `Checker::dynamic_value_consumer` carries every method call's result
    on as one dynamic expression, which the enclosing context rejects.
  - The pin folds `Bool.__bool__` (`@always_inline("builtin")`, returning
    `self`) to its receiver, so the run-time name itself meets the `or`,
    which rejects it as an operand.
  - `x.__bool__() and True` over an `Int` `x` reports the initializer at
    both, since `Int.__bool__` computes a new value.
  - Only the diagnostic's wording differs: both reject the program.
  - Found while landing R1548 (2026-11-07).
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
- The `a431ffa8af` re-pin (2026-11-01, Mojo `1.3.0.dev2026110105`)
  followed the window's renames and fixes in place: `ParameterList.length`,
  `is_ascii_whitespace`, the removed `Powable`, `CeilDivableRaising`, and
  `_Expable`, integer powers that abort on a negative exponent, the
  unordered `pop.cmp ne`, negative literal patterns, and an unread `if`'s
  `elif` call. Its unimplemented changes are R1817–R1830, and its one
  upstream regression is a probe
  (`var_case_rvalue_tuple_literal_sibling.mojo`), not an entry here.

Behavioral divergences from the pinned Mojo, to burn to zero. Every new
divergence lands here with a probe or a `cases.tsv` `mojito-only` /
`output-diff` row, and leaves when its probe promotes to an `assets/ok`
fixture. Each entry's `Ledger name` is the slug `conformance/cases.tsv` uses
for it, and an entry that waits on another names it. Five divergences are
retained on purpose and re-probed rather than fixed; they are listed in
[`docs/non-goals.md`](non-goals.md).

- [ ] **R1817 With one caller, a function over `Optional[Pointer[Int, _]]`
  prints `None` for `p.value()[]`**

  A wrong-output bug: `process(Pointer(to=x))` over `def process(p:
  Optional[Pointer[Int, _]]): print(p.value()[])` prints `None`, where
  the pin prints `5`.
  - With a second call, or a `print(Bool(p))` before the read, Mojito
    prints `5`.
  - The `_` hole nested inside `Optional` is upstream's auto-parameterized
    origin, new at the `a431ffa8af` pin (the previous pin rejected it).
  - Probe: `conformance/probes/autoparam_nested_hole_single_caller.mojo`.
  - Found at the 2026-11-01 re-pin.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1916 The bundled `StringSpan` keeps a `mut` parameter where
  upstream's origin is always immutable**

  Upstream declares `struct StringSpan[origin: ImmOrigin]`, but the bundled
  one is `StringSpan[mut: Bool, //, origin: Origin[mut=mut]]`, so a view of
  a `var` carries a mutable origin.
  - The argument-exclusivity check (`record_carried_places`,
    `checker/origins/exclusivity.rs`) reads a `StringSpan`'s origin as
    read-only by name, so `print(s.strip(), s.lstrip())` is accepted, as at
    the pin; that special case goes with the port.
  - Done when the bundled declaration is upstream's and the name check is
    gone.
  - Found while repairing the 2026-11-04 overnight gate.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1915 Unpacking a non-implicitly-copyable value into a subscript
  or field target is accepted**

  Mojito accepts `outer[0], count = (fresh^, 2)` with `fresh: List[Int]`,
  where the pin rejects it as "List[Int] cannot be implicitly copied".
  - `destructure_into_place` checks the implicit copy only for a name
    target, never a subscript or field target.
  - `ownership_test`'s
    `indexed_and_variant_payload_replacement_invalidate_nested_interiors`
    relies on the acceptance and needs respelling with the fix.
  - Found while repairing the 2026-11-04 overnight gate.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1900 A refining trait may restate its parent's parameterized
  `comptime` requirement**

  Mojito merges a refinement's `comptime M[j: Int]: Int` with its
  parent's `comptime M[k: Int]: Int`, where the pin rejects it as "invalid
  redefinition of 'M'", even when the two match in everything but names.
  - `merge_associated_requirement` (`traits_support.rs`) accepts a
    generator requirement whose signature matches the inherited one,
    names aside.
  - A restated parameterized type requirement with the same parameter
    list is merged too (`comptime T[k: Int]: AnyType` in both).
  - Ledger name: `conformance/probes/trait_refinement_restates_generator.mojo`.
  - Found while landing R1896 (2026-11-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1819 `with c:` copies a named context manager, where the pin
  enters and exits `c` itself**

  `with c:` over a local `c` copies it, so `__enter__(mut self)` mutates
  the copy, and a manager that is not `ImplicitlyCopyable` is rejected
  ("cannot be implicitly copied").
  - At `1.3.0.dev2026110105` a `with` over a reference to a manager no
    longer copies: an `ImplicitlyCopyable` manager prints no `copy`, and
    `c.n` reads `1` after an incrementing `__enter__`, where Mojito prints
    `copy` and `0`.
  - Lever: the HIR `with` lowering binds the manager by reference when the
    subject is a place, and materializes a temporary only for an rvalue.
  - Found at the 2026-11-01 re-pin.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1820 An error-taking `__exit__` cannot consume its manager**

  `def __exit__(var self, error: Error) -> Bool` on a `with Ctx():` is
  rejected ("cannot copy non-Copyable type 'Ctx' (consuming receiver of
  method '__exit__')"), where the pin runs it, as it runs `deinit self`.
  - The non-error `__exit__` already takes `var self` and `deinit self`.
  - Found at the 2026-11-01 re-pin.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1821 An integer literal to a negative literal power is a type
  error, where the pin computes a float literal**

  `print(2 ** -1)` and `(2).__pow__(-1)` fail ("invalid call to
  '__pow__': violated constraint"), where the pin prints `0.5`, and
  `(-1)**-3` is `-1.0`.
  - Upstream's `IntLiteral.__pow__` is two overloads selected by a
    `where` on the exponent's sign. The negative one returns the type of
    `1.0 / IntLiteral[base ** -exp]`, a `FloatLiteral`.
  - Port the negative overload to `stdlib/std/builtin/int_literal.mojo`
    beside the non-negative one. The pair differs only in its `where`
    clause, which R1172 owns.
  - Its signature reads `type_of(-exp)` (`IntLiteral.__neg__`, R1853) and
    divides a `FloatLiteral` by an `IntLiteral` (R1784).
  - Landing it deletes the `Pow` arm of `Checker::infer_infix`
    (`checker/operators.rs`) that reports a rejected `IntLiteral` pair's
    `__pow__` error instead of materializing it to `Int`.
  - Zero to a negative power is `nan`.
  - Found at the 2026-11-01 re-pin.
  - Depends on R1172, R1853, R1784.
  - Model: Opus, Not Planned.

- [ ] **R1822 A `match` cluster keys an unspelled `Equal` member under the
  empty spelling, where the pin no longer does**

  Since `1.3.0.dev2026110105` the pin treats an `== expr` or an implicitly
  concatenated string pattern differently from the previous pin, which
  Mojito still follows (R1140).
  - `match String("cd")` over `case "zz"`, `case "a" "b"`, `case "c" "d"`
    prints nothing in Mojito, where the pin prints `cd`.
  - `match S(2)` over `case 1`, `case == expected(2)` tests `1` first at
    the pin, where Mojito sorts the empty spelling first and calls
    `expected(2)` first.
  - Find the new keying in upstream's `findClusterSize` / `emitCaseCluster`
    at the pinned worktree, follow it, and update
    `docs/notes/pattern-matching-evidence.md`.
  - Probe: `conformance/probes/match_cluster_unspelled_members.mojo`.
    `assets/ok/match_literal_clusters.mojo` is ledgered in
    `conformance/assets-mojo-output-diffs.tsv` until this lands.
  - Found at the 2026-11-01 re-pin.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1830 Two initializers that differ only in their refined `out self`
  types are rejected, where the pin now selects by the result type**

  `def __init__(out self: W[2], *, two: Int)` beside `def __init__(out self:
  W[3], *, two: Int)` is rejected ("redefinition of function '__init__',
  cannot overload on return type"), where `1.3.0.dev2026110105` runs
  `W[2](two=7).v` and prints `7`.
  - `assets/type_error/refined_out_self_overload_on_result.mojo` pins the
    old rejection, and `conformance/assets-mojo-errors.tsv` carries it as
    `divergence`.
  - On the fix, move the fixture to `assets/ok` and drop its ledger row.
  - Found at the 2026-11-01 re-pin.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1797 An import alias of a library name the entry module also
  declares names the entry's own declaration**

  `from std.simd import Int8 as I8` beside the program's own `struct Int8`
  makes `I8(7)` construct the program's struct, where the pin's alias
  names the imported declaration.
  - Cause: `ModuleLoader` resolves the entry's imports to the library's
    bare linked names first, then renames every bare reference to an own
    declaration to `entry_item_name`
    (`crates/mojito-module/src/module.rs`), the import-resolved ones
    included.
  - Lever: rename the own declarations before the imports resolve, or
    resolve an import alias to a spelling the own-name rewrite skips.
  - Probe: the shape above, then `print(s.v, v)`, fails "does not conform
    to trait 'Writable'", where the pin prints `3 7`.
  - Probe: `from std.collections.list import List as L` beside an own
    `struct List`, then `L[Int]()`, fails "expects 0 type argument(s), got
    1".
  - Probe: `from std.simd import SIMD as V` beside an own `struct SIMD`,
    then `V[DType.int32, 4](7)`, fails "expects 0 type argument(s), got 2".
  - Probe: `from std.simd import Int as I` beside an own `struct Int`
    (reachable since R1792).
  - Found while planning R1792 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1749 A compile-time `Span` into compile-time memory materializes
  at run time, where the pin rejects it**

  `comptime s = c[1:]` over `comptime c = List([Float64(1.0), 2.0, 3.0])`,
  then `print(len(s))`, prints `2`, where the pin reports "use of a
  never-initialized interior reference 'ComptimeOrigin["element"]'".
  - Cause: `reject_display_crossing` (`checker/comptime_validation.rs`)
    rejects a crossing only for a collection or a non-`ImplicitlyCopyable`
    type, never for a value whose type's origin is compile-time memory
    (upstream's `ComptimeOrigin`).
  - Probe: `for x in s: print(x)` over the same `s` prints `2.0`, `3.0`,
    where the pin rejects it alike.
  - Probe: a module `comptime S = C[1:]` read as `print(len(S), S[0])`
    prints `2 2.0`, where the pin rejects it alike.
  - Probe: `comptime s = Span(c)` then `print(len(s))` prints `3`, where
    the pin reports "cannot materialize compile-time value of type
    'Span[Float64, ComptimeOrigin]' to a runtime value".
  - Passing the same value as a parameter (`g[s]()` over `def g[c:
    Span[Float64, _]]()`) and reading `c` in `g` runs at the pin, so the
    rule is the crossing's, not the type's.
  - Found while landing R1742 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1629 A `__getitem__(self, idx: Int)` accepts an `Indexer`, which
  the pin rejects**

  `s[Offset(3)]` over a user struct whose accessors take `idx: Int` prints
  the element, where the pin reports "invalid call to '__getitem__': value
  passed to 'idx' cannot be converted from 'Offset' to 'Int'".
  - Cause: `prepare_index_argument` (`checker/indexing.rs`) records an
    `__mlir_index__` normalization for any `Indexer` argument when an
    overload takes `Int` there and none takes the source type.
  - Upstream converts no subscript argument: a collection that takes a
    conformer declares it, as `List.__getitem__(ref self, idx:
    Some[Indexer])` does.
  - Fix: port those `Some[Indexer]` overloads to the bundled collections
    (`List`, `Span`, `InlineArray`, …), so
    `assets/ok/indexer_normalization.mojo` still runs, then delete the
    nominal arm of the normalization.
  - `SIMD`'s declared accessors already reject a conformer
    (`assets/type_error/simd_subscript_indexer.mojo`), since the
    normalization serves only a `TyKind::Struct` receiver.
  - Found while landing R1561 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1735 A bracket argument binds across a parameter list's `/` or
  bare `*` marker, which the pin rejects**

  `f[1, 2]()` and `f[a=1, b=2]()` over `def f[a: Int, /, *, b: Int = 0]()`
  print `1 2`, where the pin reports "invalid call to 'f': unexpected
  parameter" and "unexpected keyword parameter 'a'".
  - Cause: `parse_type_params` (`parser/types.rs`) skips both markers, so
    `TypeParam` records no positional-only or keyword-only boundary.
  - `match_parameter_slots` (`mojito-ast/src/call.rs`) then binds a
    positional argument to any slot and a keyword to any name.
  - Fix: record the two boundaries on the parameter list, as a `def`'s
    run-time list records `keyword_only`, and have `match_parameter_slots`
    reject a positional argument past the `*` and a keyword before the `/`.
  - Probe: `v.slice[2, 1]()` over a `SIMD[DType.int32, 4]`, whose `offset`
    is keyword-only, runs here and is rejected at the pin.
  - Found while landing R1673 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1615 A `for` over a bound declaring only `Element` iterates, where
  the pin requires `__iter__`**

  `for item in xs` over `C: Items`, a trait declaring `comptime Element` and
  no `__iter__`, runs in Mojito, where the pin rejects it: "'C' does not
  implement the '__iter__' method".
  - The checker's generic iteration accepts a bound naming an `Element`
    (`iteration_protocol`, the `TyKind::Param` arm), not only one that is
    `Iterable`.
  - Pinned by `conformance/probes/for_over_element_only_bound.mojo`.
  - `abstract_element_copyability_derives_from_member_bounds` in
    `tests/checker_test.rs` uses the accepted form; rewrite it over an
    `Iterable` bound when this lands.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1987 A `while True` or a `try` that never falls through does not
  end its block**

  The code after a `while True` that only returns, or a `try` whose body
  and handlers all leave, stays live, so an instance judges its `comptime
  assert`s, where the pin erases it as dead.
  - `CheckedTables::ends_block` (`mojito-checked/src/checked.rs`) ends a
    block at an `if` or `comptime if` whose every arm leaves, but never at
    a loop or `try`.
  - Probe: `def k[n: Int]() -> Int` holding `while True: return 1`, then
    `comptime assert n == 5, "dead"`, fails `k[1]()` with "constraint
    failed: dead", where the pin prints `1`.
  - Probe: `try: return 1` with `except: return 2`, then the same
    assertion, in a `raises` function: the same failure, and the pin
    prints `1`.
  - Read which loops upstream's `LowerSemanticCF` follows with
    `hlcf.unreachable` before widening the rule; R1242 owns the
    move-scan side of the same dead loop exit.
  - Found while landing R1587 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1498 A module outside the bundled library is rooted at its file
  stem, not its dotted name**

  Problem: `def f[T: Foo](var x: T): pass`, with `trait Foo` imported from
  `zpkg/aa.mojo` (`from zpkg.aa import Foo`), names `'Foo'` as the abandoned
  type, where the pin names `'AnyType'`.
  - Upstream orders a composition's traits by fully qualified symbol
    (`sortAndDeduplicateTraitSymbols`), rooted at the module's dotted name
    (`zpkg.aa`), so `zpkg` sorts after `std`.
  - `module_symbol_path` (`checker/traits.rs`) reads only a statement's
    stamped file path, which names the dotted module for a bundled module
    and the entry module (its stem) alone.
  - The lever: the linker's `rooted_module_name` knows each loaded module's
    dotted name; stamp or carry it beside the path.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1499 `@explicit_destroy` on a `trait` does not parse**

  Problem: `@explicit_destroy("baz must be closed") trait Baz: pass`
  stops with "a decorator must precede a 'def' or 'struct'", where the pin
  accepts it and names `baz must be closed` when a `T: Baz` value is
  abandoned.
  - Upstream sets the trait's `LinearTypeErrorMsg` from the decorator
    (`DeclResolution.cpp`), and rejects it on a trait conforming to
    `Deinitable`.
  - `Checker::linear_trait_message` (`checker/traits.rs`) gives every trait
    the default `unhandled explicitly destroyed type '<Trait>'`; it would
    read a message `TraitInfo` records.
  - Probe: `def f[T: Baz](var x: T): pass` with the trait above.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1439 A `try` whose first raise is a `String` catches an
  `Error`**

  `try: raise s` for a `String` `s`, then `except e: var t: String = e`, is
  rejected ("expected String, found Error"), where the pin prints `dyn`:
  upstream types a `try`'s error as its first raise's type, as
  `parseRaiseStmt` resolves `__try_error__`, but Mojito's checker types
  the `except` binding as `Error`.
  - MIR lowering already types the slot from the first raise
    (`Flatten::raise_exit`), so the checker and the slot disagree.
  - Both backends stage a store into a `try`'s slot by converting a string
    to an `Error` whatever the slot's type (`VmBackend::stage_error`,
    Pliron's `FnLowering::stage_error`), where `raise_exit` converts only
    into an `Error`-typed slot.
  - Probe: in a raising function, `try: raise s`, `except e: print(e);
    raise e` stops at elaboration ("unsupported monomorphizing
    `String.__moveinit__`: expected `String`, found `Error`"), where the
    pin prints `dyn` twice.
  - Probe: the same with a bare `except: print("caught"); raise
    Error("again")` fails in the VM ("field access on non-struct Error"),
    where the pin prints `caught` and `again`.
  - Found while landing R1437 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1128 `main()` may raise only `Error`**

  Problem: `def main() raises StopIteration:` is accepted and traps at run
  time, where the pin rejects the declaration ("'main()' can only raise
  'Error'; remove the explicit error type").
  - `assets/runtime_error/pliron_raise_iter_stop_unhandled.mojo` pins the
    native unhandled-error text through exactly this spelling; once `main`
    rejects a typed `raises`, a helper raises `StopIteration` and `main`
    lets it escape through a plain `raises`, or the fixture retires.
  - Ledger: `conformance/assets-mojo-errors.tsv` carries the fixture as
    `skip` with the pin's complaint.
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1130 A t-string reads its interpolations when it is written**

  Problem: `var t = t"x={x}"` then `x = 2` then `print(t)` prints `x=1` in
  Mojito, which snapshots each interpolation at creation, where the pin
  prints `x=2`: its `TString` holds references into the origins it is
  parameterized on and formats what they hold at write time.
  - The previous pin printed `x=1` too, so this is the window's change;
    `assets/ok/tstring_lazy.mojo` is the ledger row (`one-off`), and
    `conformance/parity.tsv` `types.string-literal-families` records the
    representation difference.
  - Probe: `print(t"{S(3).v}")` over an `S` printing in its destructor
    prints `del 3` before `3`. The pin's `TString` borrows the call
    temporary, so it lives until the `print` returns (`3`, then `del 3`).
  - Lever: an origin-parameterized reference pack on the bundled `TString`
    (`TString[origins, //, *Ts]`), which the capability ledger has long
    named as the missing shape.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1144 A `var` case binding that is a named subject's last use stays
  a copy**

  Problem: `match t: case var b if guard(b.v): ... case _: ...` with no
  later use of `t` prints `copy 1`, two `del 1`s, then `guard 1` in
  Mojito, where the pin moves `t` into `b` and prints one `del 1` before
  `guard 1`.
  - The subject is bound to a hidden `$match` reference to `t`, and the
    case's `var b` copies from `t` inside the `match` region's case body.
  - `analysis/copy_elision.rs` turns a copy into a move only when the
    source's `DropVar` follows the construction directly, and no scan
    reaches past a region op other than `try`; `t` dies after the `match`.
  - Over a temporary subject and a plain `var u = t`, the last-use copy
    already moves as at the pin
    (`assets/ok/match_binding_kinds_on_temporary.mojo`,
    `assets/ok/match_guard_copy_drop.mojo`,
    `assets/ok/implicit_copy_last_use_forwards.mojo`).
  - Probe: `conformance/probes/match_temporary_subject_survives_failed_var_case.mojo`,
    whose third `match` is this shape;
    `assets/ok/match_guards_and_var_bindings.mojo` reads the subject after
    every match to stay clear of it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1239 A `ref` binding of a temporary subject's element is
  mutable**

  Problem: `match (String("a"), 1): case (ref s, _): s += "!"` prints `a!`
  in Mojito, where the pin rejects the write ("expression must be mutable
  for in-place operator destination").
  - The pin reads a tuple or sequence element through `__getitem__` on the
    rvalue subject, so its `ref` result is immutable; a root `as` binding
    and a field projection stay mutable on both.
  - R1133 already reads an enum payload of a temporary immutably
    (`MatchSubject::rvalue`, `checker/patterns.rs`); tuple and sequence
    element reads need the same rule.
  - Ledger name: `conformance/probes/match_temporary_element_mutable.mojo`.
  - Found while landing R1133 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1225 A `var` or `ref` nested under a declaring target prefix
  declares a name**

  Problem: after `var [c, var d] = values` or `var (ref a, b) = pair`,
  Mojito declares `d` and `a`, where at the pin the nested convention
  declares nothing and a later use is `use of unknown declaration 'a'`.
  - The parser distributes the outer `var` to every leaf and keeps a
    leaf's own convention (`parse_target_item` in
    `crates/mojito-parser/src/parser/patterns.rs`), so the nested leaf
    becomes an ordinary declaration.
  - Lever: reject a nested convention when the prefix already declares;
    Mojito may reject what the pin only accepts unused.
  - Probe: `var (ref a, b) = pair; print(a, b)` runs and prints `1 2`,
    since `a` exists.
  - Probe: `for var (var a, b) in pairs: print(a, b)` runs, where the pin
    warns "nested 'var' or 'ref' patterns are redundant" and rejects the
    read with "use of unknown declaration 'a'"; the loop target shares the
    parser's distribution.
  - Found while planning R1136 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1131 A temporary handed to an owning collector runs its move
  initializer**

  Problem: `idx(M(copy=k))` with `def idx(var *xs: M)` prints `i 9 1` and
  `del 9` in Mojito, where the pin prints `i 1009 1` and `del 1009`: the
  temporary is moved into the pack through `__init__(out self, *, deinit
  move: Self)`, and Mojito hands its storage over without a call.
  - Ledger name: `assets/ok/variadic_collector_forwarded_and_borrowed.mojo`
    in `conformance/assets-mojo-output-diffs.tsv` (`one-off`). The `^`
    transfers of named locals in the same fixture agree on both.
  - Both pins behave alike, so this is an unoracled Mojito divergence.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1100 A free function with an origin binder over `Span[T, o]`
  is rejected at its call**

  Problem: `def first[T: ImplicitlyCopyable & Movable, o: Origin](s:
  Span[T, o]) -> T` called as `first(Span(l))` stops with "type mismatch
  for argument to 'first': expected Span[Int], found Span[Int]", where the
  pin prints the element.
  - The two spellings differ only in their origin argument, the binder `o`
    against the span's inferred origin, which the call should bind.
  - Probe: `/tmp`-style program of the two lines above over
    `var l = List[Int]()` with two elements, printing `first(Span(l))`.
  - Found while planning R1078 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R606 A diagnostic names an entry declaration by its linked name**

  Problem: an error about the entry's own `struct List` says
  `'__module$$$main$$$List'`, where the pin says `'List'`.
  - The entry's declaration of a name the library links bare, a builtin
    identity, or a prelude name takes the entry module's linked name
    (`module::entry_item_name`), and the checker prints a struct's linked
    name as it is.
  - Library types leak the same way (`__module$list$List`).
  - Probe: the library `ContiguousSlice` and `StridedSlice` print as
    `__module$std$builtin$builtin$slice$ContiguousSlice` in every
    diagnostic (`assets/type_error/contiguous_slice_not_equatable.mojo`,
    `conformance/fixtures/slice_kind_does_not_widen.mojo`,
    `conformance/fixtures/strided_slice_has_no_bounds.mojo`).
  - Probe: an overload failure of a library function names its linked
    symbol: `max(2, UInt8(1))` reports "invalid call to
    '__module$std$math$math$max'" (`call_inference.rs` passes the linked name to
    `TypeError::BadCall`), where the pin says "no matching function in call
    to 'max'" (`assets/type_error/max_literal_first_mismatch.mojo`).
    `exp(SIMD[DType.int32, 2](1, 2))` and `log` over an integral dtype
    say '__module$std$math$math$exp' and '…$log' the same way
    (`assets/type_error/math_exp_integral_dtype.mojo`,
    `assets/type_error/math_log_integral_dtype.mojo`).
  - The lever: diagnostics spell a type through `unqualified_type_name`,
    as runtime type names already do.
  - Found while landing R605 (2026-10-10).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R744 A conditional expression copies a place arm that is not
  `ImplicitlyCopyable`**

  Problem: `take(f if c else M(13))`, with `M` only `Copyable`, compiles
  and copies `f` silently, where the pin very likely rejects the arm with
  "value of type 'M' cannot be implicitly copied".
  - Mojito rejects the same implicit copy at a binding (`var h = f`) and at
    a list-subscript arm.
  - The checker types each arm with plain `infer` and marks a place arm
    `copy_place_value_use` without asking for `ImplicitlyCopyable`.
  - Fix: run each arm through `Checker::check_consuming_as` in the
    `ExprKind::IfExpr` arm, after a pin probe confirms the rejection, and
    add an `assets/type_error` fixture with its
    `conformance/assets-mojo-errors.tsv` row.
  - Unverified on the pin: the pin env was missing when R735 landed.
  - Found while landing R735 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1019 A user `Writer` receives other `write_string` calls than on
  the pin**

  Problem: `w.write(-23)` calls a user writer's `write_string` once with
  `-23` on Mojito, but three times (`-`, an empty string, `23`) on the pin,
  and `w.write(1.5)` makes one call on Mojito and four on the pin.
  - A writer that counts or frames its calls prints different output.
  - The bundled scalar `write_to`s write whole values, where upstream's
    write sign and digit pieces separately.
  - Fix: respell the bundled `Int` and float `write_to`s after
    upstream's, then promote a probe to an `assets/ok` fixture. `Bool`'s
    is upstream's since R1611, and `w.write(True)` writes `True` once, as
    at the pin.
  - Found while planning R1003 (2026-10-15).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R902 `read` is accepted as an argument name**

  Problem: `def same(read: Int) -> Int` compiles and runs, where the pin
  rejects it: "'read' was removed; use 'imm'" and "expected argument name".
  - Mojito accepts a program the pin rejects, so this is a divergence.
  - Ledger name: `read-argument-name`
    (`conformance/fixtures/read_argument_name.mojo`).
  - The fix rejects `read` in the parser's argument-name position with the
    pin's message, and moves the case to an `assets/parse_error` fixture.
  - Found while landing R789 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R925 A recursive `@always_inline` function is accepted**

  Problem: a reached `@always_inline` function that calls itself, directly
  or through other `@always_inline` functions, compiles and runs, where the
  pin rejects it: "function has recursive call to 'always_inline'
  function", with a note at the recursing call.
  - Mojito accepts a program the pin rejects, so this is a divergence.
  - Ledger name: `recursive-always-inline`
    (`conformance/fixtures/recursive_always_inline.mojo`).
  - The pin rejects plain, `nodebug`, and `builtin` alike, and only when
    the function is reached; `inline-parametric` already finds such cycles
    and keeps their calls.
  - Fix: report the cycle as an error when an elaboration root reaches it,
    and move the case to an error fixture.
  - Found while landing R784 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R888 A conditional expression over two places copies the chosen
  place where upstream yields a reference to it**

  `show(a if c else b)`, with `a` and `b` named `ImplicitlyCopyable`
  values and `show` taking a `read` argument, prints only `show 1` on the
  pin, and Mojito prints `copy 1`, `show 1`, `del 1`.
  - Upstream's `mergeCValuesAcrossIfLikeOp` (`IREmitter.cpp`, around line
    1565) yields a common reference from `handleTwoMValues` when both arms
    are lvalues that dominate the `if`, so nothing is copied.
  - `var r = a if c else b` copies once on both, so only a borrowing
    consumer differs.
  - Lever: the checker's `ExprKind::IfExpr` arm stops marking place arms
    `copy_place_value_use` when both arms are places, and
    `Flatten::ternary` yields `MakeRef` handles from its `If`.
  - Found while landing R871 (2026-10-14), probe on the pin
    `mojo-2026092105`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R860 A `TrivialRegisterPassable` struct may define a copy
  constructor or `__del__`, or hold a non-trivial field, and a
  `RegisterPassable` one a move constructor or a memory-only field**

  Mojito accepts all three, and runs the user copy constructor and
  destructor, where the pin rejects each at the declaration.
  - The pin's errors: "trivial types must not declare explicit copy
    constructors", "trivial types must not declare '__del__' methods", and
    "all members of 'TrivialRegisterPassable' struct must themselves be
    'TrivialRegisterPassable'" (the trait's docstring in
    `std/builtin/value.mojo` states the same).
  - The checker should reject them where it checks a struct's declared
    conformances.
  - Once it does, `MirDeclarations::copy_is_identity` can trust the
    declaration and drop its `__copyinit__`, `__del__`, and field walk.
  - Pinned by `conformance/probes/trivial_register_passable_custom_copy.mojo`,
    `trivial_register_passable_custom_del.mojo`, and
    `trivial_register_passable_non_trivial_field.mojo`.
  - Probe (from R1396): `struct B[n: Int](Copyable, TrivialRegisterPassable
    where n > 0)` is accepted, where the pin rejects it: "conditional
    conformance to 'TrivialRegisterPassable' is not supported". Its erased
    instance `B[_REMOVED_ARG]` drops the row its concrete instances keep,
    so the VM's `build_structs` boxes every instance of a template whose
    instances disagree. That rule goes with the rejection.
  - Probe (from R1524): `struct C(Copyable, RegisterPassable)` with
    `__init__(out self, *, deinit move: Self)` runs the user move
    constructor, where the pin rejects it: "'RegisterPassable' types must
    not declare explicit move constructors".
  - Probe (from R1524): `struct S(Movable, RegisterPassable){s: String}`
    runs, where the pin rejects it: "all members of 'RegisterPassable'
    struct must themselves be 'RegisterPassable'".
  - Found while landing R838 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.
- [ ] **R1453 Bundled types do not declare upstream's `RegisterPassable`
  and `TrivialRegisterPassable`**

  Problem: `conforms_to(T, TrivialRegisterPassable)` is `False` in Mojito
  and `True` at the pin for a `StringSpan` or a `range(n)` result, and
  likewise for every bundled type whose upstream declaration lists one of
  the two.
  - `TrivialRegisterPassable`: `StringSpan`, `Layout`, `ErrNo`,
    `_CTimeSpec`, `StopIteration`, and the three `range` structs.
  - `RegisterPassable`: `EmptyOptionalError`, `OwnedPointer`, `TString`,
    `Allocation`, `ThinAllocation`, and `Variant where
    Ts.all_conforms_to[RegisterPassable]()`.
  - Cause: either declaration changes how the value passes natively, so
    the native ABI rows in `docs/native-abi.md` must follow, and the
    checker does not yet enforce `TrivialRegisterPassable`'s rules at a
    declaration (R860).
  - `Span` already declares `TrivialRegisterPassable`.
  - Found while landing R578 (2026-10-26).
  - Depends on R860 and R1226.
  - Model: Opus, Not Planned.

- [ ] **R1454 Bundled types lack the members and traits upstream's other
  declared conformances need**

  Problem: a few bundled types omit a conformance upstream's library
  declares, because the bundled type lacks the member the trait requires
  or the bundle lacks the trait, so `conforms_to` answers `False` where the
  pin answers `True`.
  - `Writable` (a `write_to`): `Layout`, `Allocation`, `ThinAllocation`,
    `StopIteration`, `OwnedPointer where T: Writable`, and `StringDict
    where V: Writable`.
  - `IterableOwned` on `Dict`: an owning `__iter__(var self)` and its
    iterator. Upstream's clause reads `KeyElement` (R1171); `K: Deinitable`
    spells it meanwhile.
  - `Defaultable` and `Hashable where T: Hashable` on `Span`: an empty
    constructor and a `__hash__`.
  - `ReversibleRange` on the three `range` structs: the bundle has no such
    trait, so `reversed(range(n))` has an overload of its own
    (`stdlib/std/builtin/reversed.mojo`).
  - Probe: `comptime if conforms_to(T, IterableOwned)` at `T =
    Dict[String, Int]` holds at the pin, not in Mojito.
  - Found while landing R578 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R525 Materialization errors omit Mojo's contextual notes**

  The crossing diagnostic has one message where Mojo distinguishes a bare read from a call argument.
  - A bare read suggests `materialize`; a call argument suggests evaluating the whole call with `comptime`.
  - `TypeError` has no diagnostic-note channel.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R442 `TString` is in scope in user code, where the pin has no such
  name**

  Problem: `TString(*args^)` in a user `def` runs in Mojito, where the pin
  rejects it with "use of unknown declaration 'TString'".
  - The bundled `TString` struct is visible as a prelude name, though the pin
    keeps its t-string type out of the user's namespace.
  - No t-string needs the name: the checker calls `__make_tstring` by
    module path, so the prelude export only serves user spellings.
  - Found while landing R316 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R2041 A struct method's `origin_of(self)` result is not bound to
  its receiver, so a returned view of the wrong argument converts**

  `def make(ref value: Box, ref other: Box) -> View[origin_of(value)]:
  return other.view()`, over `def view(ref self) -> View[origin_of(self)]`,
  runs here, where the pin rejects it with "cannot implicitly convert
  'View[origin_of(other)]' value to 'View[origin_of(value)]'".
  - The method call's result type keeps `Origin::SelfParam` in its tail:
    `record_call_result_origins` (`checker/origins/actuals.rs`) records no
    view-return slot for the call, so `bind_call_result_tail` leaves it.
  - The same return through a free function (`mk(other)` over `def
    mk(ref b: Box) -> View[origin_of(b)]`) is rejected as at the pin.
  - Probe: the program above, which should fail to check.
  - Found while landing R1805 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1153 An owned-interior origin converts to its base origin**

  Problem: a value over `origin_of(xs)._get_owned_interior["element"]`
  converts silently to the same type over `origin_of(xs)`, where the pin
  rejects the conversion ("cannot implicitly convert 'Span[Int,
  origin_of(xs["element"])]' value to 'Span[Int, origin_of(xs)]'").
  - Probe: `conformance/probes/list_slice_returned_as_whole_origin.mojo`,
    a `List` slice returned as `Span[Int, origin_of(xs)]`.
  - Probe: `conformance/probes/string_view_returned_as_whole_origin.mojo`,
    `s.rstrip()` returned as `StringSlice[origin_of(s)]`.
  - Not root-caused: `Origin::coerces_to` itself keeps the interior path
    (`mojito-types/src/origin.rs`), so the acceptance lies elsewhere.
  - Found while landing R190 (2026-10-19).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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
    and a SIMD lane read `ref lane = v[i]`, which binds a materialized copy
    of the lane `SIMD.__getitem__` returns.
  - Probe: `ref r = s[0]` over a user `__getitem__` returning `Int32`, and
    `ref q = t[1]` over one returning `Int`, bind a materialized copy, where
    the pin rejects both with the same text.
  - A memory-backed temporary (`ref x = make_list()`) is accepted by both.
  - The lever is the `StmtKind::RefDecl` arm in
    `crates/mojito-checker/src/checker/statements.rs`, whose
    `materialized_reference_actual` fallback materializes any value.
  - Pinned by `conformance/probes/ref_binding_register_value.mojo`.
  - Probe: `ref z = Int()` runs in Mojito, where the pin rejects it: "value
    of type 'Int' doesn't have a memory origin in 'ref' binding". A `match`
    already applies the rule to `case ref z` over a temporary subject
    (`bind_subject` in `checker/patterns.rs`, `TypeError::NoMemoryOrigin`,
    `assets/type_error/match_ref_binding_register_temporary.mojo`); add
    the plain form beside that fixture.
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

- [ ] **R171 Four corpus fixtures compute different numbers from the pin at
  arithmetic edge cases such as an over-wide shift**

  Problem: four corpus fixtures compute different numbers from the pin,
  found by the 2026-09-12 stdout sweep and listed in
  [`conformance/assets-mojo-output-diffs.tsv`](../conformance/assets-mojo-output-diffs.tsv)
  under `arithmetic`.
  - The clearest is a shift past the bit width
    (`assets/ok/pliron_straightline.mojo`: `a << 65`, where the pin prints
    7 and Mojito 51102306).
  - The SIMD shift and floor-division fixtures disagree wholesale.
  - Also here: an out-of-range float-to-int cast.
  - `round()`'s half-way case left this list on 2026-09-13, when `round`
    became ties-to-even on both backends.
  - Each case needs the pin's rule established before Mojito's is changed.
  - `docs/native-abi.md` already defines some of them deliberately (wrapping
    overflow), so the plan decides which are bugs and which are recorded
    choices.
  - Ledger name: `native-arithmetic-edge-cases`.
  - `simd_floordiv_mod.mojo` differs only at `MIN // -1`, a lane the pin's
    compile-time fold makes `0` and Mojito leaves `MIN`. At run time the
    pin prints `-128` for an `Int8` lane, as Mojito does, and crashes
    (SIGFPE) on `Int`, so the fixture's row pins LLVM's undefined case,
    not a zero-divisor rule.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R981 A callable parameter's default naming a function declared
  later is rejected**

  Problem: `def g[f: def(Int) thin -> Int = double](x: Int)` declared
  above `def double` is rejected ("'double' has type undefined and is not
  callable") at the call `g(1)`, while the pin runs it.
  - Declaring `double` first makes it work, so the default is resolved
    against the functions declared so far rather than the module's.
  - Found while landing R910 (2026-10-15).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R1155 `Span`'s data pointer and `List.unsafe_ptr` name the owned
  interior, so `Span(list)` is invalidated where the pin's is not**

  Problem: `var ys = Span(xs); xs.clear(); print(len(ys))` is rejected
  ("access to 'xs' conflicts with live reference 'ys'"), where the pin
  prints `3`.
  - Mojito's `Span._data` is `Pointer[T, origin._get_owned_interior[
    "element"]]` and `List.unsafe_ptr()` returns the interior pointer;
    upstream's `_data` is `Pointer[T, origin]`, `unsafe_ptr()` returns
    `Pointer[T, origin]`, and a separate `_unsafe_interior_ptr()` serves
    `List.__getitem__(ContiguousSlice)`.
  - Upstream's `Span(list)` rebinds the list's pointer to
    `origin_of(list)` (a TODO there), so the view does not borrow the
    elements' generation.
  - A slice span `Span[T, interior(o)]` therefore carries a doubly
    interior data pointer inside Mojito; no probe observes it.
  - Probe: `conformance/probes/span_of_list_survives_clear.mojo`.
  - Probe: a `Pointer` over an owned interior does not keep its owner
    alive, though a `Span` or `ref` over one does. `var p =
    xs.unsafe_ptr(); print(p[0].id)` over a `List` of a printing
    destructor destroys `xs` before the read, where the pin reads first.
    `OwnedPointer.unsafe_ptr` returning upstream's `Pointer[T, origin]`
    keeps its owner alive, so returning `origin_of(self)` is the fix.
  - Probe: `var s = String("hello"); var bytes = s.as_bytes(); f(bytes.
    unsafe_ptr(), 5)`, with `f` reading the pointer, traps with "use after
    Pointer deallocation", where the pin reads `h`. The bundled
    `_debug_assert_fail_format` passes the `String`'s own pointer instead
    (`stdlib/std/builtin/debug_assert.mojo`'s header note).
  - Found while landing R190 (2026-10-19).
  - Depends on R177.
  - Model: Fable, Not Planned.

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
  - Probe: the bundled `Iterable` (`stdlib/std/iter.mojo`) declares
    `comptime Element`, which the pin's does not, so `def first[C:
    Iterable](items: C, default: C.Element) -> C.Element` is accepted
    where the pin reports "'Iterable' value has no attribute 'Element'".
    Snippets in `tests/vm_test.rs` and `tests/mir_test.rs` use that
    signature.
  - The bundled `List.__init__` from an iterable copies the iterable's
    `Element` and `rebind`s it to `type_of(self).T`, where upstream's
    `for var value in iterable` moves the iterator's element in with
    `rebind_var` (R1737). Restore upstream's body when both land.
  - Probe: the same copying body in a user initializer, `for value in
    iterable: var element = value.copy()`, runs in Mojito, where the pin
    reports "'IterableType.IteratorType[...].Element' value has no
    attribute 'copy'".
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

- [ ] **R182 A struct's parametric `comptime` member over a bare
  parametric struct is accepted, and a return typed through it escapes the
  returned-origin check**

  Problem: `comptime ViewType[m: Bool, //, o: Origin[mut=m]] = View` inside
  `struct Box`, with `def view(ref self) -> Self.ViewType[origin_of(self)]`
  returning a view over `self.items`, runs in Mojito; the pin rejects the
  use with "'View[_]' is not concrete, use '[]' to bind missing
  parameters".
  - The member's parameters never reach `View`, so the return type is
    the unbound `View` and the returned `View[origin_of(self.items)]`
    passes. Spelled `-> View[origin_of(self)]` directly, or through `=
    View[o]`, the same return is rejected as at the pin, for a `ref`-field
    `View` and its `Pointer`-field twin alike
    (`assets/type_error/return_origin_widening_field_view.mojo`).
  - The member is resolved by `resolve_parameterized_member`
    (`checker/type_resolution.rs`), which should reject a body naming a
    parametric struct without its parameters, as a module-level alias's
    application already stops (on its origin parameter).
  - Probe: the `Pointer`-field `View` of the fixture above, returned through
    `comptime ViewType[m: Bool, //, o: Origin[mut=m]] = View`, prints `7`.
  - Five `assets/extensions/ok` fixtures return through such a member and
    widen: `ref_field_view_for_temporary.mojo`,
    `ref_field_view_ref_yield.mojo`, `ref_field_view_method_return.mojo`,
    `ref_field_drain_mut_method.mojo`, and
    `ref_field_chained_view_call.mojo`. Respell each member `= View[o]`
    and each return over the field's own origin.
  - Ledger name: `ref-field-return-origin-widening`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R188 `sp[byte=i]` and `s[codepoint=i]` yield a byte value and a
  `Codepoint`, where the pin yields one-byte and one-codepoint views**

  Problem: `sp[byte=i]` reads the byte value in Mojito and returns the
  one-byte view upstream. `s[codepoint=i]` is a `Codepoint` in Mojito and a
  one-codepoint `StringSpan` upstream.
  - The same shape as `string-subscript-element` (R183), on the keyword
    subscripts.
  - Declaration changes in the stdlib's `String`/`StringSpan`.
  - Ledger name: `string-span-byte-index`, `string-codepoint-index`.
  - Probe: `s[byte=Int32(1)]` and `s[codepoint=UInt8(2)]` over a `String`
    match no overload, where upstream's `byte=` and `codepoint=` overloads
    take an `[I: Indexer, //]` binder and the pin prints `b` and `c`; the
    bundled ones take `Int` only (found while landing R1564, 2026-11-07).
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
  - `StringLiteral.split` forwards to `StringSpan.split`, so it returns
    `List[String]` too, where upstream's returns `List[StaticString]`.
  - An API shape change with display and iteration fallout across every
    `split` caller.
  - Ledger name: `split-returns-owned-strings`.
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

- [ ] **R349 Two packs in one parameter list are accepted**

  Problem: `def joined_len[*As: Movable, *Bs: Movable](a: Tuple[*As], b:
  Tuple[*Bs])` runs in Mojito, where the pin stops with "cannot have two
  '*' markers in the same parameter list".
  - A method's own pack beside its struct's pack is legal at the pin.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R350 `deinit` on a parameter that is not of `Self` type is accepted**

  Problem: `def take(self, deinit other: Lin)` on a struct other than `Lin`
  is accepted in Mojito, where the pin stops with "'deinit' must only be
  applied to arguments of Self type".
  - `Tuple.concat`'s `deinit other: Tuple[*OtherTs]` is the legal shape:
    another instance of the same struct.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R356 A generic `def` returning a concatenation of its own pack with
  itself is accepted**

  Problem: `def doubled[*Ts: ImplicitlyCopyable](t: Tuple[*Ts]) ->
  Tuple[*TypeList._concat[Ts.values, Ts.values]()]: return t.concat(t)` runs
  in Mojito, where the pin stops with "cannot implicitly convert
  'Tuple[*#kgen.param_list.concat(Ts.values, Ts.values)]' value to" the same
  spelling.
  - Mojito closes both lists to one canonical concatenation, so the result
    and the annotation are one type.
  - `flip`'s `Tuple[*Ts.reverse()]` result is accepted by both.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R404 A raising call in a compile-time collection literal is
  accepted**

  Problem: `comptime for x in [twice(n), n]:` over a `def twice(n: Int)
  raises -> Int` runs in Mojito, in a generic `def` or a method, where the
  pin stops with "cannot call raising function in collection literal".
  - A runtime list literal (`var l = [twice(1), 2]` in a raising body) is
    accepted by both.
  - Found while landing R400 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R408 A `Variant[*Self.Ts]` field is accepted over a pack not
  bounded `Deinitable`**

  Problem: `struct Box[*Ts: Copyable]` holding `var v: Variant[*Self.Ts]`
  compiles and runs in Mojito, where the pin rejects the field: "field 'v'
  has non-'Deinitable' type 'Variant[*Ts.values]'".
  - `Variant` is `Deinitable` only where every alternative is, and the pin
    demands that of every field at the declaration.
  - Pinned by `conformance/probes/variant_pack_field_not_deinitable.mojo`.
  - Ledger name: `variant-pack-field-not-deinitable`.
  - Found while landing R4 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1463 A `Pointer` conforms to `Writable` but cannot be printed**

  Problem: `print(Pointer(to=x))` is rejected ("does not conform to trait
  'Writable'"), where the pin prints the address, though
  `modelled_declares` answers `Writable` for `TyKind::Pointer`, as
  upstream's `Pointer` declares.
  - Neither backend has a display of one: the bundled library declares
    no `Pointer.write_to`.
  - Upstream's `write_to` writes the address in hex and `write_repr_to`
    writes `Pointer[mut=True, SIMD[DType.int, 1],
    address_space=AddressSpace.GENERIC](0x...)`.
  - Probe: `repr(Span(xs))`, which takes `Writable`'s synthesized default
    until R761, writes its `_data` field as `Pointer(0x11+0)` on the VM and
    stops natively with "unsupported display of a Pointer".
  - Found while landing R424 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R425 A malformed literal `format` template aborts at run time where
  the pin rejects it at compile time**

  Problem: `"[{:>5}]".format(3)` and `"{} {}".format(1)` fail to compile at
  the pin, while Mojito compiles them and aborts when the call runs.
  - Upstream's `StringLiteral.format` parses its template at compile time
    (`_FormatUtils.format_to_comptime[StaticString(Self())]`) and fails a
    `comptime assert` with the parse error.
  - `StringLiteral.format` is upstream's body, but the bundled
    `_FormatUtils.format_to_comptime` in `stdlib/std/string.mojo` parses
    at run time and aborts with the same message; a path the run never
    takes is accepted.
  - Ledger: `format-literal-template-runtime`
    (`conformance/fixtures/format_literal_template_runtime.mojo`).
  - Lever: port upstream's `format_to_comptime`, whose `comptime result =
    compile_entries_runtime_no_raises[*Ts](format)` and `comptime assert`
    reject a bad template while the program compiles.
  - Found while landing R421 (2026-10-06).
  - Depends on R213.
  - Model: Fable, Not Planned.

- [ ] **R514 `rebind` between distinct structs of one lowered layout is
  rejected, where the pin accepts it**

  Problem: `rebind[S](x)` with `x: U`, where `U` and `S` each hold one
  `Int` field under any names, fails the instance in Mojito ("rebind input
  type 'U' does not match result type 'S'"), while the pin runs it by
  reference and by value.
  - The pin's `kgen.rebind` compares lowered KGEN types; Mojito's
    `discharge_rebinds` (`mono/rebind.rs`) compares checked types with
    `types_agree`, which is nominal.
  - The pin still rejects a different layout (`UInt` or `Float64` against
    `Int`, `String` against `S`), as Mojito does.
  - Following the pin needs a lowered-type identity in the elaborator,
    from `native-core`'s layouts, which judges the value and place forms
    alike.
  - Probe: `rebind[Float32](__mlir_attr.`#kgen.simd<"inf"> :
    !kgen.scalar<f32>`)` prints `inf` at the pin, a scalar's storage and
    its `SIMD` lowering alike. Mojito rejects the closed rebind in the
    checker first (`Checker::rebound_ty`, `checker/rebind.rs`: "the input
    type does not match the result type"). The pin rejects
    `!kgen.scalar<f32>` into `Float64` ("rebind input type
    '!kgen.scalar<f32>' does not match result type '!kgen.scalar<f64>'").
  - Pinned by `conformance/probes/rebind_layout_identical_struct.mojo`.
  - Ledger name: `rebind-layout-identical-struct`.
  - Found while landing R314 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R515 Assigning through a whole `rebind` destroys the old value at
  the operand's type, where the pin destroys it at the target's**

  Problem: `rebind[S](x) = S(9)` with `x: T`, `T: Copyable`, is rejected
  in Mojito ("'x' abandoned without being explicitly destroyed"), while the
  pin prints `9`.
  - The pin assigns through the reference its `ref` overload returns,
    typed `S`, so the old value is destroyed as an `S`.
  - Mojito's `erase_rebinds` rewrites the statement into the plain
    assignment `x = S(9)`, whose destruction is judged at `T`, which is not
    `Deinitable`.
  - Following the pin means lowering the statement as a store through the
    rebound place (its `Proj::Rebind` step) rather than rewriting it.
  - Pinned by `conformance/probes/rebind_assignment_without_deinitable_bound.mojo`.
  - Ledger name: `rebind-assignment-without-deinitable-bound`.
  - Found while landing R314 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R511 A negative literal index into an `Array` traps at run time
  where the pin rejects it at compile time**

  Problem: `var a = [1, 2, 3]; print(a[-1])` fails to compile at the pin
  ("constraint failed: negative indexing is not supported"), while Mojito
  compiles it and traps with "Pointer access out of bounds".
  - Upstream dispatches a subscript whose index is a compile-time constant
    to `Array.__getitem_param__[idx]`, whose `comptime assert` rejects a
    negative or out-of-range index.
  - Mojito's bundled `Array` has no `__getitem_param__`, and the checker
    never routes a constant subscript to one.
  - Pinned by `conformance/probes/array_negative_literal_index.mojo`.
  - Probe: `comptime q = [1, 2][i]` under `comptime for i in
    range(k)` in a method instantiated at `k = 5`
    (`assets/type_error/struct_method_per_call_clone_fails.mojo`) fails as
    a VM pointer fault in the lifted thunk, where the pin fails the
    `__getitem_param__` constraint "index is out of bounds".
  - Ledger name: `array-negative-literal-index`.
  - R2043 waits on this: an `Array` display's subscript in a signature
    becomes an application once the getter exists.
  - Found while landing R274 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1432 A closure's storage copies a `{var u}` capture once where it
  is built, where the pin copies it twice**

  A `def inner() {var u}` declaration and a lambda `{var u}` passed to a
  `F: def() -> Int` binder, over an `ImplicitlyCopyable` struct, each
  print `copy 4` once where Mojito builds the storage; the pin prints it
  twice, and once for an immediately invoked lambda, as Mojito does.
  - Upstream's storage `__init__` takes a copied capture as `ImmMem` and
    copies it into its field (`liftClosure`'s initializer,
    `emitStoreToLValue`); Mojito's synthesized `__init__` takes the
    capture's value once (`build_closure_storage`, `mir/lower_stmt.rs`).
  - Root-cause the immediately invoked lambda's single copy at the pin
    before changing the initializer's convention.
  - Probe: `conformance/probes/closure_value_copies_its_storage.mojo`
    (`copy 4` twice on the value line at the pin, once on both backends).
  - Probe: `print(once(lambda {var u} -> Int: u.n))`, then `def inner()
    {var u} -> Int: return u.n`, over `u = Tracked(4)`.
  - Found while landing R1378 (2026-10-25).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1869 A postfix `^` after a closing parenthesis does not parse**

  `var t = (s)^` over a `String` local, and `var q = (lambda {var u} ->
  Int: u.n)^`, which the pin's own diagnostic suggests for a lambda whose
  storage cannot be implicitly copied, both stop with "Expected an
  expression"; the pin runs both.
  - `parse_infix` (`parser/exprs.rs`) reads `^` as the transfer sigil only
    when it abuts `left`'s span, and a parenthesized expression's node
    keeps its inner span, so the sigil after `)` reads as `^` xor.
  - A transfer of a lambda then moves its storage variable, so the check
    must admit it where `check_consuming_as` rejects the copy.
  - Probe: the two bindings above.
  - Found while landing R1435 (2026-11-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1890 A struct's generic `__call__` does not conform to a
  generic closure trait with value binders**

  `struct Adder(def[n: Int](Int) -> Int)` declaring `def __call__[n:
  Int](self, value: Int) -> Int` is rejected where a binder takes it
  ("declared callable contract 'def(Int) -> Int' is incompatible"); the pin
  prints `43` for `invoke(Adder(1))` calling `f[2](40)`.
  - The struct's declared contract is lowered by `ty_from_anno` in
    `Checker::resolve_struct_member_types` (`checker/traits.rs`), which
    keeps origin binders as reference signatures and drops a value
    binder, so the recorded `callable_conformance` is monomorphic.
  - A parameter's callable bound goes through
    `lower_anonymous_callable_type` instead and keeps its `def[...]`
    binders; the conformance needs the same lowering, and the `__call__`
    check against it must compare the binders.
  - Probe: the shape above, with `invoke[F: def[n: Int](Int) -> Int](f:
    F) -> Int: return f[2](40)`.
  - Found while landing R1823 (2026-11-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1772 A bare `def(...)` run-time parameter type is a concrete
  function type, where the pin's is a closure trait**

  The pin reads a run-time parameter annotated `def(...)` with no `thin`
  as a trait, as it reads one on a field or an element;
  Mojito checks it as a function type and calls through it.
  - Probe: `def run(h: def(Int32) -> None)` called `run(show)` over `def
    show(v: Int32)` prints `3` here; the pin rejects the argument ("cannot
    be converted from 'def show(v: Int32) thin -> None' to 'def(Int32) ->
    None'") and the call in the body ("value cannot be converted from type
    value '$0' to an instance of '$0'").
  - Probe: the same parameter over a binder, `def run[d: DType](h:
    def(Scalar[d]) -> None)` called `run[DType.int32](show)`, passes the
    check and fails MIR verification ("argument 0 of 'run' has type
    def(Int32) thin -> None, declared def(Scalar[d]) -> None"), as does a
    method's `h: def(Scalar[Self.dtype]) -> None` on `C[DType.int32]`.
  - Reject the annotation at the parameter, as fields and elements are
    rejected; do not close `verify_direct_call`'s declared types under
    the call's arguments, which would make the binder shapes run.
  - Found while landing R1767 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R567 A user `std/` directory merges into the bundled `std`, where
  the pin binds `std` to its library package alone**

  `from std.mine import mine` runs in Mojito when `std/mine.mojo` sits
  beside the entry file or under a `-I` root; the pin rejects it ("unable
  to locate module 'mine'") in both cases.
  - Mojito probes every root for the whole dotted path, so a user
    `std/x.mojo` also shadows the library's `std.x`.
  - Following the pin means binding the top-level `std` once, to the
    library package, before any user root is searched for its members.
  - Reproduce with a scratch directory holding `std/mine.mojo` and the
    importing file; no probe is checked in, since a `std/` beside the
    probes would reach every other probe's imports.
  - Found while landing R565 (2026-10-10).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R741 Mojito accepts `c^ + d^` as two transfers, which the pin
  rejects**

  Problem: `var s = c^ + d^` over a struct `__add__(var self, var other:
  Self)` runs in Mojito, where the pin lexes `^ +` as XOR followed by
  unary plus and reports "'M' does not implement the '__pos__' method".
  - `(c^) + (d^)` is the spelling both accept.
  - Probe: `conformance/probes/transfer_operand_before_infix_operator.mojo`.
  - Found while landing R730 (2026-10-13).
  - Depends on R1569, which parses unary `+`.
  - Model: Opus, Not Planned.

- [ ] **R815 Mojito accepts a `Dict` entry's key as its caller's bound,
  which the pin rejects**

  Problem: in a generic `def` whose `K` is `Writable`, `print(e.key)` over
  `d.items()` prints in Mojito, where the pin reports "an element of
  'values' with type 'K' does not conform to trait 'Writable'".
  - At the pin the field reads as `DictEntry`'s own `K`, bound by its
    `KeyElement`-shaped declaration; `d.keys()` or a copy into a `var kk: K`
    is accepted by both.
  - Probe: `conformance/probes/dict_entry_key_keeps_caller_bound.mojo`.
  - Found while landing R774 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R817 The prelude exports `Set`, which the pin's does not**

  Problem: `Set[Int]()` without an import runs in Mojito, where the pin
  reports "use of unknown declaration 'Set'; did you mean to import it
  from 'std.collections'?".
  - `stdlib/std/prelude.mojo` imports `Set`; upstream's prelude does not.
  - Removing it means adding `from std.collections import Set` to every
    fixture and bundled module that spells it bare.
  - Probe: `conformance/probes/set_without_import.mojo`.
  - Probe: `conformance/probes/optional_reg_without_import.mojo`.
    `OptionalReg` is a prelude name the pin's prelude lacks too ("did you
    mean to import it from 'std.collections'?").
  - Probe: `conformance/probes/is_trivially_without_import.mojo`.
    `IsTriviallyCopyable[Int]` runs without an import, since the checker
    recognizes the `IsTrivially*` spellings (`types::trivial_predicate_name`)
    whatever is in scope; the pin asks for `from std.traits import …`.
  - Found while landing R774 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R851 A constant out-of-range shift runs instead of failing to
  compile**

  Problem: `print(UInt8(128) >> UInt8(9))` prints `64` in Mojito, while the
  pin fails to compile with "could not simplify operator shr(128, 9)".
  - The VM masks a lane shift amount to the lane width.
  - `param_expr::fold` refuses the shift, so `sccp` leaves it to the VM.
  - Matching the pin needs a compile-time diagnostic when a shift's
    constant amount is outside the lane width.
  - Its home is the folded `pop.shr` of a library `SIMD.__rshift__`
    (R1526).
  - Found while landing R846 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1089 A `raises` destructor is accepted where the pin rejects it**

  `def __deinit__(deinit self) raises:` checks and runs in Mojito, where
  the pin reports "destructor must not declare 'raises'".
  - Probe: a struct whose `__deinit__` declares `raises`, dropped in
    `main`.
  - The checker's method rules say nothing about a destructor's effect;
    `mir::lower_drops` writes the destructor's call with no raise
    contract, and Pliron rejects a raising destructor at the drop.
  - Reject the declaration at the check, with the pin's message.
  - Found while landing R1043 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1215 `unsafe_origin_cast` accepts a target of different
  mutability**

  Problem: `self.data.unsafe_origin_cast[origin_of(self)]()` under `ref
  self`, over a `Pointer[Int, MutUntrackedOrigin]` field, is accepted,
  where the pin rejects it: "value passed to 'target_origin' cannot be
  converted from 'Origin[mut=self_is_mut]' to 'MutOrigin'".
  - Upstream declares `unsafe_origin_cast[target_origin: Origin[mut=mut]]`:
    the target keeps the pointer's own mutability, and `unsafe_mut_cast`
    is the spelling that changes it.
  - Mojito's `unsafe_origin_cast` arm
    (`crates/mojito-checker/src/checker/method_calls/builtin_types.rs`)
    weakens a mutable target to an immutable pointer's capability instead
    of matching the two.
  - The same body under `mut self` is accepted by both.
  - Found while landing R679 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1324 A struct over a place origin converts implicitly to one over
  an unsafe any-origin**

  `var box: Borrowed[MutUnsafeAnyOrigin] = Borrowed(UnsafePointer(to=x))`
  compiles, where the pin rejects it: "cannot implicitly convert
  'Borrowed[origin_of(x)]' value to 'Borrowed[MutUnsafeAnyOrigin]'".
  - The two `Borrowed` rebinding tests of `tests/drops_test.rs` rely on the
    conversion. Respelling them with an explicit `unsafe_origin_cast` needs
    the owners kept alive first.
  - Found in the 2026-10-24 overnight-gate triage.
  - Depends on R1323, which the respelled tests need.
  - Model: Opus, Not Planned.

- [ ] **R1906 The bundled owned `List` iterator holds a raw buffer, where
  upstream's holds the drained `List`**

  `reflect[type_of(it)]` over `var it = xs^.__iter__()` names
  `_ListOwnedIter` with fields `data`, `size`, and `index`, where the pin
  names `_ListIterOwned` holding `_list: List[T]` and `_index`.
  - Lever: `_ListOwnedIter` and `List.__iter__(var self)` in
    `stdlib/std/collections/list.mojo`, and `Set.__iter__(var self)` in
    `set.mojo`, which build it from the list's leaked `_storage`.
  - Upstream's `__deinit__` destroys the untaken tail and zeroes
    `_list._len` so the held `List` frees only its buffer.
  - Found while landing R1482 (2026-11-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1510 `type_of`, `origin_of`, and `conforms_to` are names, where
  upstream lexes them as keywords**

  Problem: `def type_of(x: Int) -> Int` runs in Mojito, where the pin says
  "'type_of' cannot be used as a function name in this context".
  - Upstream: `kw_type_of`, `kw_origin_of`, and `kw_conforms_to` reach
    `ExprParser::parseMagicFunction`, which builds a `MagicFunctionNode`.
  - Mojito parses each as an `ExprKind::Call` (or `Type::TypeOf`) and
    matches the name: `build_ct`'s `type_of` arm, `build_ct_rest`'s
    `conforms_to`, and the parser's `origin_of`.
  - The lever: lex the three as keywords and give the AST one magic-call
    node.
  - Found while landing R1369 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2048 A builtin body passing an argument to a `read` parameter of
  a type that is not trivially register-passable is not rejected**

  The pin's fold binds a `lit.ref.immut` to such an argument and rejects
  it, before it judges the call, whatever type the call instantiates the
  parameter at.
  - Probe: `return Int(b)` over a `b: Bool` runs, where the pin says "does
    not support MLIR operation lit.ref.immut": `Int(b)` calls
    `__init__[T: Intable](out self: Scalar[target_dtype], value: T)`.
  - Probe: `return abs(n)`, `return abs(3)`, and `g(n)` over
    `g[T: AnyType](x: T)`, `g[T: RegisterPassable](x: T)`, or
    `g[T: Copyable](x: T, y: Int)` are rejected with "only supports calls
    to other '@always_inline("builtin")' functions", where the pin says
    "does not support MLIR operation lit.ref.immut"; `g[T:
    TrivialRegisterPassable](x: T)` takes the "only supports calls"
    message at both.
  - The lever: `Checker::passes_by_reference` (`checker/builtin_fold.rs`)
    judges the collector only. Extend `declared_collector` to the
    callee's declared parameter types and conventions (`MethodSig`, the
    bound `FuncTy`), rejecting a `read` one that `passes_in_register`
    refuses.
  - That rule misjudges the six `Bool(__mlir_attr[...] : i1)` bodies of
    `stdlib/std/builtin/float_literal.mojo` until R2047 lands: the call
    resolves to `Bool.__init__[T: Boolable]`.
  - Found while landing R1991 (2026-11-08).
  - Depends on R2047.
  - Model: Opus, Not Planned.

- [ ] **R2049 A builtin body's callee is judged by the name the call
  spells, not by the overload the check resolves**

  `Checker::body_operations` (`checker/builtin_fold.rs`) flags a call
  before the check only when no overload of the spelled name is builtin
  (`calls_ordinary_function`, `calls_ordinary_method`), and the
  statement and operator rules judge the body's syntax before its body
  check, where the pin's fold walks the checked ops in order.
  - Probe: `return h(n, n)`, where `h(n: Int)` is builtin and `h(n: Int,
    m: Int)` is not, runs and prints `6`, where the pin says "only
    supports calls to other '@always_inline("builtin")' functions".
  - Probe (after R2048): `var x = Int(b)` before a `while` is rejected
    with "lit.loop", where the pin rejects the earlier `lit.ref.immut`.
  - The lever: give `Checker::builtin_call_rejection` the resolved
    callee's inline level (the target `declared_collector` finds), and
    move the statement and operator rules into the same ordered walk.
  - Found while landing R1991 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1992 A run-time prefix operator over a compile-time `SIMD`
  vector is not folded, so it judges its builtin dunder's `comptime
  assert`**

  `var x = -b` over a `comptime b = SIMD[DType.bool, 4](True, False, True,
  False)` prints `[False, True, False, True]` at the pin, which folds the
  builtin `SIMD.__neg__` over its parameter operand and drops its numeric
  assertion. Mojito fails `main`'s instance: "the SIMD type must be
  numeric".
  - `Checker::record_folded_simd_operator` folds a run-time infix
    operator over parameter operands, as upstream's parser folds a builtin
    call, but no prefix one.
  - Folding the prefix node needs two levers: MIR's parameter-constant
    read (`call_family` in `lower_expr/expr.rs`) takes no `Prefix` node,
    and the elaborator evaluates no prefix parameter expression over a
    vector, so `-b` keeps the constant "in elaborated MIR after
    monomorphization" and `~b`, folded to `b ^ -1`, meets R1960.
  - Probe: `print(~a)` and `print(-SIMD[DType.int8, 2](3))` over
    `comptime a = SIMD[DType.int32, 4](1, 2, 3, 4)` must keep printing
    `[-2, -3, -4, -5]` and `[-3, -3]` once prefix nodes fold.
  - Found while landing R1527 (2026-11-07).
  - Depends on R1960.
  - Model: Opus, Not Planned.

- [ ] **R1656 A construction that leaves a struct's origin parameter
  uninferred is accepted**

  `V(5)` over `struct V[mut: Bool, //, origin: Origin[mut=mut]]` with
  `def __init__(out self, k: Int)` prints its field here, where the pin
  rejects it with "failed to infer parameter 'mut' of parent struct 'V'".
  - `OriginBindings::bind_tail` (`checker/origins/construct.rs`) leaves an
    origin slot no argument binds as `Origin::Unbound`, and the
    construction proceeds.
  - Reject the construction when a slot stays unbound and no explicit
    application supplies it, as upstream's inference does.
  - Probe: `V[origin_of(l)](5)` over a local `l` runs at both.
  - Found while landing R1192 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1852 A tuple display converts element-wise to any tuple-typed
  context, where the pin converts it only at a binding**

  `g((1, 2.5))` over `def g(t: Tuple[Int8, Float32])` runs here, where the
  pin deduces `Tuple[Int, Float64]` first and rejects it with "value passed
  to 't' cannot be converted from 'Tuple[Int, Float64]'".
  - `infer_with_expected`'s contextual `TupleLit` arm
    (`checker/inference.rs`) types each element against any tuple-typed
    expected type, and call, method, operator, and list-display binding
    all pass their parameter type as that context.
  - The pin keeps the element-wise context only for a `var` annotation,
    an assignment target, a `return`, and a nested tuple display.
  - Probe: `g((1, 2.0))` over `def g(t: Tuple[Float64, Float64])`.
  - Probe: `List[Tuple[Int8, Int8]]().append((1, 2))`.
  - Probe: `var l: List[Tuple[Int8, Int8]] = [(1, 2)]` ("no matching
    function in initialization" at the pin).
  - Probe: `t == (1, 2)` over `t: Tuple[Int8, Int8]`.
  - A list display keeps its context in argument position: `f([1, 2])`
    into `List[Int8]` runs at both.
  - Lever: give the arm its context only from the binding, assignment,
    `return`, and nested-display callers; the other callers infer the
    display plainly and reach the pin's rejection.
  - Found while landing R1786 (2026-11-02).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1885 `NoneType` conforms to `Writable`, where the pin's does not**

  `print(None)` and `f[T: Writable](None)` run here, where the pin rejects
  them: "an element of 'values' with type 'None' does not conform to
  trait 'Writable'".
  - `mojito_types::conformance::modelled_conformance` lists `Writable` for
    every modelled leaf but a callable, `TyKind::None` among them.
  - Probe: `String(None)` prints `None`, where the pin rejects it: "an
    element of 'args' with type 'None' does not conform to trait
    'Writable'".
  - Found while planning R1287 (2026-11-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1913 A type binder bound to a value-annotated `comptime` name
  is accepted**

  `comptime x: Int = T` in `def go[T: Copyable]` runs here, where the pin
  rejects it: "cannot implicitly convert 'Copyable' value to 'Int'".
  - The checker judges a `comptime` initializer against its annotation
    only as a value; a type binder's value is its metatype (the trait it
    is bounded by), which converts to no value type.
  - Probe: `comptime x: Int = T` over `T: Copyable`.
  - Probe: `comptime x: Int = Ts[0]` over `*Ts: Copyable`, the same
    message at the pin.
  - Found while landing R1492 (2026-11-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1980 A failed `assert` prints no `At: <location>: ` prefix**

  A failing `debug_assert[assert_mode="safe"](False, "boom")` prints
  `Assert Error: boom` and traps, where the pin prints
  `At: <file>:<line>:<col>: Assert Error: boom`.
  - The bundled `_debug_assert_msg` prints through `print` and traps through
    `abort(message)`, where upstream's calls `_printf` and the no-argument
    `abort() -> Never`.
  - `_debug_assert_fail_format` writes into a `String`, where upstream's
    writes into a stack `_FixedWriteBuffer`.
  - Port `_printf`, `_FixedWriteBuffer`, and `abort()` verbatim, then make
    `_debug_assert_msg` and `_debug_assert_fail_format` upstream's
    (`stdlib/std/builtin/debug_assert.mojo`'s header note).
  - Probe: `assets/runtime_error/debug_assert_safe_mode_fails.mojo`.
  - Found while landing R1598 (2026-11-06).
  - Depends on R1979 (the location), R50 and R799 (`abort() -> Never`).
  - Model: Opus, Not Planned.

- [ ] **R1983 A multi-byte `ascii_rjust` fill character traps where the
  pin's default mode skips upstream's `assert`**

  `String("hi").ascii_rjust(6, "é")` traps with "fill char needs to be a one
  byte literal", where the pin skips `_justify`'s `assert` under its default
  `ASSERT` mode and pads with the fill's first byte (`\xc3` four times, then
  `hi`).
  - The bundled `_justify` (`stdlib/std/string.mojo`) checks with
    `_mojito_abort`, written before the `assert` statement parsed.
  - Spell upstream's `assert fillchar.byte_length() == 1, "fill char needs
    to be a one byte literal"` there, and move
    `assets/runtime_error/nominal_string_justify_fillchar.mojo` to an `ok`
    fixture with its manifest rows.
  - Found while landing R1598 (2026-11-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2014 `String.write_repr_to` writes its quoted text in one write,
  where upstream's writes each piece**

  Upstream's `StringSpan.write_repr_to` writes the opening quote, each
  codepoint or escape, and the closing quote as separate `write_string`
  calls. The bundled `String.write_repr_to` (`stdlib/std/string.mojo`)
  builds the whole text in a buffer first.
  - A writer that marks each call sees the difference.
  - Upstream also writes a non-printable ASCII byte as a `\x..` escape
    (`_write_hex`), which the bundled body does not.
  - Probe: `e.write_to(w); e.write_repr_to(w)` over `Error("boom")` into a
    writer that writes `|` before each piece gives `|boom|Error|(|'boom'|)`,
    where the pin gives `|boom|Error|(|'|b|o|o|m|'|)`.
  - Found while landing R1938 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2034 A non-power-of-two `SIMD` vector the pin's code generation
  rejects runs**

  The pin admits `SIMD[DType.int32, 3]` as a type and folds a construction
  of one, but its LLVM lowering rejects every `!kgen.simd` value of a
  non-power-of-two length that survives its optimizer: "SIMD vector length
  must be a power of two between 1 and 2^15".
  - Mojito's stand-in is the MIR verifier's width rule
    (`verify_simd_instruction`, `mir/verify/instr.rs`), which checks only
    `simd.make`, `simd.cast`, and `simd.bitcast`, so a vector built by the
    element list initializer or carried through arithmetic, a call, a
    field, or a `List` runs on both backends.
  - The rule must not reach a value the pin folds or deletes first:
    `takes_three(SIMD[DType.float32, 3](1.0))` over an unused parameter,
    and `SIMD[DType.int32, 3](7)[2]`, print at the pin
    (`assets/ok/simd_non_power_of_two_width.mojo`).
  - Probe: `conformance/probes/simd_non_power_of_two_vector_survives.mojo`
    (`print(w + w)` over a three-lane `w`) prints `[2, 4, 6]`.
  - Probe: `print(w)`, `w.reduce_add()`, `w[1] = 9`, a function returning
    `SIMD[DType.int32, 3]`, a struct field of that type, and
    `List[SIMD[DType.int32, 3]]` all run, where the pin rejects each.
  - Found while landing R1734 (2026-11-08).
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

- [ ] **R1809 `SIMD.reduce_add`, declared `@inline(.always)`, stays an
  out-of-line call**

  A caller of `v.reduce_add()` keeps a call to the library's split-tree
  instance, so a native caller spills the vector to pass it by pointer,
  where upstream inlines the body at every call.
  - Seen in `simd_lowering_emits_vector_code`
    (`tests/pliron_backend_test.rs`), whose fixture now calls
    `reduce_add` where it was a builtin rule.
  - Find which pass declines the splice (`inline-parametric` or the
    post-elaboration `automatic-inline`) and why; the body recurses through
    `slice` on halves.
  - Found while fixing the 2026-11-01 overnight gate.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R867 A conditional's literal arm reaches its carrier
  unmaterialized**

  `x if c else 1` stores an `IntLiteral` register into an `Int` carrier
  slot (`$scN`), where upstream converts the arm to the expression's type
  at the arm.
  - Seen in `String.__init__(capacity_bytes=)`
    (`stdlib/std/string.mojo`), `StringSpan._split_on`, and `_atof_scan`.
  - Until R865 the instance's after-the-fact retyping flipped such slots
    between `Int` and `IntLiteral` by store order; now they keep the
    generic slot type and the mismatch stays in the stored register.
  - The fix is a `MaterializeLiteral` on the literal arm in lowering; a
    structural verifier rule that a `DefVar` source matches its slot type
    then pins it.
  - Found while landing R865 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1269 The VM reads a whole-slot place through a reference handle
  the slot holds, so `sroa` keeps a struct with a pointer field**

  The VM holds a `Pointer(to=x)` as a frame reference handle
  (`Value::Ref`), and any place rooted at a slot holding one goes through
  the handle (`place_handle`, `crates/mojito-vm/src/backend/vm/references.rs`),
  so a pointer field moved into its own slot reads its pointee where the
  field read the pointer.
  - `sroa` refuses a struct with a pointer field for this reason
    (`splittable_type`), where upstream's `SROA` splits it.
  - A slot's place should read through a handle only when the slot is a
    `ref` binding, which the place's `through` and the slot's type say,
    not because of the value it holds.
  - Probe: dropping the pointer test in `splittable_type` makes
    `assets/ok/keyed_def_builds_loan_carrying_instance.mojo` fail with
    "use of variable slot 0 after it was moved": the split `Box[Pointer]`
    field's `drop.place` takes `xs` through the handle.
  - `sroa` keeps a struct with a `ref` field whole for the same reason:
    a place past the split field would need `through: Some(slot)` and a
    `ref` root type, where the rewrite leaves `through: absent`, which
    Pliron reads as the referent itself (native SIGSEGV on
    `assets/extensions/ok/ref_field_written_nodebug_init.mojo`).
  - Probe (from R1396): a one-field `TrivialRegisterPassable` struct whose
    field is a reference or a pointer stays a boxed `Value::Struct` in the
    VM (`build_structs`), where the pin holds it as the pointer. Held as
    its field, `S[o]{p: UnsafePointer[Int, o]}` over `UnsafePointer(to=x)`
    fails `s.get()` with "ReadRef r0 received Int(5)", since its slot then
    holds the `Value::Ref` the VM takes for an alias.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1308 An inlined initializer's `out self` is a slot of its own the
  caller forwards, where upstream binds the caller's destination**

  `inline-parametric` gives a constructed receiver (`Binding::Out`) a fresh
  slot and forwards it into the caller's `var.store` (`Returned::Out`),
  where upstream's inliner initializes the caller's allocation in place.
  - Two allocations where the pin has one leave `self$inline1` whole,
    as `self.show()` borrows it, beside a split `w`, where the pin's one
    allocation stays whole under that borrow.
  - Probe: `tests/mir_passes/sroa/forwarded_from_kept_slot.mir`, from
    `var w = W(3)` over an `@always_inline` initializer that calls
    `self.show()`, keeps `self$inline1` and splits `w`, where the pin has
    one allocation.
  - Found while landing R1267 (2026-10-23).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1335 The MIR verifier does not reject an owned register no path
  ends**

  `verify/drops.rs` rejects an owned register ended twice, but not one
  that reaches an exit unended, where upstream's `DestructorInserter`
  leaves no owned SSA value undestroyed.
  - The 2026-10-24 census found one shape: an owned `Error` read in a
    `try` handler no raise reaches. Since R1326 that read is a
    `place.load` of the handler's error slot, which `drop.var` ends, so
    the handlers of `pliron_reference_write_back`,
    `copyable_iterator_reference_aggregate`,
    `read_temporary_conditional_argument_drop`,
    `try_finally_raise_skips_own_handler`, and
    `try_region_shadowed_binding` hold no owned register (2026-10-26), and
    no handler erasure is needed.
  - The rule: report an owned register live at a function or region exit.
  - Before landing it, re-run the census: the rule on, `run` over
    `assets/ok` and `assets/ownership_ok`, with `MOJITO_PRE_ELABORATION=0`
    too, since the rule runs after every pass in a debug build. File any
    new shape's cause.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1419 MIR reads a call operand's convention off the checker's
  argument markers, where upstream reads the callee's signature**

  `Flatten::operand_c` (`mir/call_results.rs`) reads or takes a call-result
  slot by the checker's `ReadTemporaryArgument` and `ForwardedArgument`
  markers (`checker/places.rs`), which restate the selected callee's
  conventions.
  - A method call's contract already carries them
    (`CheckedCallContract::arguments`, `receiver_convention`), but a free
    call records no contract.
  - An operand neither marker names falls back to taking its slot.
  - Fix: record each call's argument conventions in its checked contract,
    read them in `operand_c`, and delete the two markers; the package
    format moves.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1918 A method call passes its receiver and by-address arguments
  as loaded values beside their places**

  `mir::address_call_operands` rewrites only `MirInstr::Call`, so
  `call.method` still passes a whole load of a `read` receiver or
  argument of a memory-only type, and of a `mut`/`ref` one, with the place
  in `recv_place` and `arg_places`.
  - A `call` naming a receiver-bearing method by its symbol
    (`Pt.method(p)`) is skipped too
    (`mir::call_operand_parameters` answers `None` for it).
  - Upstream has one call operation, whose receiver operand is a
    `!lit.ref` like any other by-address argument.
  - Fix: apply the same rewrite to `call.method`'s receiver and
    arguments, and teach the VM's method binding (`vm/frames.rs`,
    `vm/invoke.rs`) and Pliron's `lower/methods.rs` to read an addressed
    operand.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1931 The loan check reconstructs a call argument's convention
  from declaration tables, so a trait-dispatched method's and a keyword
  slot's retained places are unclassified**

  `loan_accesses` (`analysis/loans.rs`) asks `CalleeRefParams` for the
  slot's `param_writes`, which a `__trait_dispatch` symbol, an unresolved
  `call.method`, and a keyword place never answer, so their places are
  `LoanAccess::Unclassified` and exclude a shared live loan as a write
  would; upstream's `lit.call` carries each operand's convention in its
  callee's type.
  - Probe: `writer.write_string(span[i])` over a live shared `Span`
    reference of the same storage (`__trait_dispatch.write_string$ov$…`).
  - Probe: a keyword argument `f(x=a)` at a `read` slot beside a live
    shared reference to `a`.
  - Fix: record each argument's convention on `call`/`call.method` at
    lowering (the checker binds it), and classify by that record.
  - Found while landing R1930 (2026-11-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1919 An indirect call, a subscript, and a type-parameter
  construction pass by-address operands as loaded values beside their
  places**

  `call.indirect`, `index`, `index.multi`, `index.multi_set`, and
  `construct_type_param` keep the value-plus-place operand shape that a
  direct `call` no longer has.
  - Their `arg_places`, `base_place`, `index_place`, and `kwarg_places`
    tables are each read by the VM and Pliron to recover the address.
  - Fix: extend `mir::address_call_operands` to each instruction in turn,
    with the VM's and Pliron's binding for it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1920 A call keeps `arg_places` beside its address operand, and
  the backends recover the address from the place**

  Upstream's `lit.call` has no side table of places: the operand is the
  address.
  - The VM loads an addressed operand's place back into a value
    (`addressed_by`, `vm/exec.rs`) and rebinds a reference parameter from
    the place (`reference_argument`), and Pliron reads the place
    (`place_backed_arg_value`, `lower/abi.rs`).
  - The verifier accepts either a reference or a value at a non-owned
    slot (`verify_direct_call`), so an artifact written before the
    rewrite still runs.
  - The rewrite runs after lowering (`mir::address_call_operands`, called
    from `lower_program` and the package's unit lowering), where upstream
    emits the address as the operand is lowered.
  - A place ending in a `ref` field keeps its load, which reads the stored
    handle's referent.
  - The inliner's lookbacks (`loaded_place`, `read_through_register`,
    `inline_parametric.rs`) still reverse-engineer a place for an operand
    with none.
  - Fix: emit the `ref.make` in `lower_call_operand`, derive each place
    from the operand's `ref.make`, keep only a per-operand "lends to the
    result" bit for `loans.rs`, delete the place-based argument paths, make
    the verifier require a reference at a by-address slot, and move the MIR
    text version with an upgrade of older calls.
  - Depends on R1917, R1918, R1919.
  - Model: Fable, Planned.

- [ ] **R1420 A raising call's register-passable result stays a register,
  where upstream returns every raising callee's result through memory**

  Upstream's signature (`Signatures.cpp`) makes a throwing function's ABI
  result its error flag, so its result always goes through a synthesized
  `ByRefResult`, beside the `ByRefError` the error takes.
  - `Checker::record_result_conventions` judges only the declared result
    type, so `print(atol(s))` keeps its `Int` in a register.
  - Mojito models a raise as an edge (`MirInstr::raise_edge`), so the slot
    changes no behavior: it is the structure upstream's error ABI implies.
  - Fix: count a raising callee's result as returned through memory, and
    update the MIR tests that pin instruction adjacency next to a raising
    call (`owned_operand_transfers_at_the_call_after_a_raising_operand`,
    `tests/mir_test.rs`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2045 Five MIR pass test inputs predate the verifier's
  memory-only register rule and fail at load**

  Each input destroys a memory-only register directly, which the MIR
  verifier has rejected since R1873 (`deinit` receiver calls) and R1932
  (unnamed `drop.reg`), so `tests/mir_pass_test.rs` fails before the pass
  runs.
  - `tuple_reverse_closed`: `swapped`'s `deinit` receiver call destroys
    a memory-only `Tuple` register.
  - `elaborate-drops/register_drop_named.mir` ("drop.reg destroys %r2 of
    memory-only type Noisy").
  - `elaborate-drops/register_drop_unnamed.mir` drops a memory-only
    `Error` register; it fails first with "unknown type `Error`", and
    needs a register-passable value instead.
  - `inline-parametric/deinit_register_receiver.mir` and
    `read_owned_register_temporary.mir`: a `deinit` receiver call
    destroys a memory-only `Box`/`Res` register.
  - Fix: re-emit each input from current lowering with `emit-mir`, which
    since R1934 (2026-11-08) ends such a register through a slot, and
    re-bless the `.expected` files, which R1837 and R1849 rewrote by hand.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2046 With the library linked from source, a compile-time call
  passing a `String` to a `ref` parameter reads an unresolved value**

  `MOJITO_STD_PACKAGE=0 mojito run
  assets/ok/comptime_application_ref_parameter.mojo` fails with "unsupported
  compile-time application `bc` reads an unresolved value
  `__module$std$string$String("abc", "abc")`", where the package-linked run
  prints the pin's output.
  - The application is `bc(String("abc"))` in `H`'s `comptime m`, `bc`
    taking `ref s: String`.
  - It was hidden until R1934 (2026-11-08): the source-linked run failed
    verification of `CompilationTarget.unsupported_target_error` first.
  - Probe: the same run with the package linked, and compare.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R607 The bundled publics keep bare linked names the compiler
  recognizes by spelling**

  The library's `List`, `range`, `Writer`, `Slice`, and their kin link
  under their bare names, and the checker finds them by that spelling,
  while upstream names every declaration by its module
  (`std.collections.list.List`).
  - `implicit_public_identity` and `builtin_module_exports`
    (`crates/mojito-module/src/module.rs`) keep the names bare; checker
    sites match `"List"`, `"String"`, and others directly.
  - The entry's own declaration of such a name is qualified instead
    (`module::entry_item_name`, R605), so a program can shadow one, but no
    other module's declarations are.
  - Running a library file as the entry fails for the same reason:
    `mojito run stdlib/std/collections/optional.mojo` reports an origin
    error, and `list.mojo`, `string.mojo`, and `dict.mojo` fail their
    checks, since the file's declarations are renamed while the checker
    still recognizes them by spelling.
  - Found while landing R605 (2026-10-10).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1422 A slice literal is not an expression node**

  Mojito's parser spreads a slice literal over `ExprKind::Slice`,
  `SubscriptArg::Slice`, and `SubscriptArg::KeywordSlice`, where upstream's
  `SliceLiteralNode` is one expression, an operand of its subscript, that
  becomes `T(lower, upper, stride, __slice_literal__=None)` once its
  parameter type is known (`SliceLiteralNode::emitIR`).
  - So the checker stands a literal in with a checker-only `ExprKind::None`
    argument keyed by a derived syntax id, and keeps its operands in a side
    table (`Checker::slice_literals`, `synthetic_slice_descriptor`).
  - MIR rebuilds the constructor call from the recorded
    `CheckedSliceLiteral` (`slice_literal_argument`), where upstream's call
    is an ordinary checked node.
  - The lever: one `ExprKind::SliceLiteral { lower, upper, step }` operand,
    checked against its expected type as an initializer, so the subscript
    forms keep plain `Index` and keyword arguments.
  - The side table's names (`SliceDescriptors`, `subscript_descriptor`,
    `synthetic_slice_descriptor`) still say "descriptor", from before
    `Slice` was a library struct; they go with it.
  - `CheckedSliceLiteral::indirect_result` goes with it too: the literal's
    node then records its own `IndirectResult`.
  - Probe: the rebuilt constructor's `Optional` operands carry no
    checked node, so `xs[::2]` keeps each `Optional.__init__` result in a
    register a `drop.reg` destroys, where upstream reads it in a
    `__call_result_tmp__` (`assets/ok/slice.mojo`,
    `list_strided_slice_normalizes.mojo`).
  - Depends on R1514, which makes every bracket an index of expressions.
  - Model: Fable, Not Planned.

- [ ] **R1423 MIR wraps each subscript argument in a one-variant
  `MirSubscriptArg`**

  `MirSubscriptArg` is only `Index(Reg)` now that a slice literal is a
  constructor call, where upstream's subscript is a call whose operands are
  plain values.
  - Collapse it to `Reg` in `MultiIndex`/`MultiSet` `args` and `kwargs`
    and in the text format's `index(...)` argument (a MIR text bump), about
    100 matches.
  - Pliron's `SubscriptActual` has one variant too, and
    `unused_params::Operand` keeps an `Option<Reg>` that no operand leaves
    empty; both collapse with it.
  - Found while landing R1158 (2026-10-25).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1284 Compile-time `len` of a compile-time-only operand is a
  checker query, where upstream applies the library `len`**

  `len(Ts)` over a type pack, `len(tl)` over a `TypeList` binding,
  `len(Self.values)` over a raw list, and `len(names)` over a reflected
  list are answered by checker queries, where the pin applies its library
  `len` to the operand's value at compile time (its `Ts` is a `TypeList`
  value, whose `__len__` the `Sized` overload calls).
  - A run-time `len(Ts)` or `len(tl)` is the library call over the
    spelled `TypeList` value already; only the compile-time folds remain.
  - The comptime evaluators fold `len` by name: `eval_reflection_expr`,
    `pack_length_binder`, the bound-display arm, and
    `compile_dependent_ct_expr`'s raw-list arm (`checker/constraints.rs`,
    `checker/reflection.rs`), and `applies_callable` excludes it
    (`checker/comptime_requests.rs`).
  - The VM's and Pliron's bare `len` callee survives for the pack cursor's
    has-next test (`mir::lower_stmt::emit_has_next`) and CTFE's
    compile-time lists.
  - Done when each operand's length is a compile-time application of the
    library overload and the bare `len` callee is gone.
  - Probe: R804's `Box[len(vs)]`.
  - Probe: a user struct's member `comptime size: Int = len(Self.values)`
    over `*values: Int`, read as `PL[1, 2, 3].size`, fails with "not an
    associated comptime expression", where the pin prints `3`: a member
    has no body to lift the call from, and the check answers `len` of a
    raw list only inside the bundled `ParameterList`.
  - `called_application` applies a function to a struct-valued argument
    since R1528 (a member `comptime count = mylen(Foo())` runs), so a
    member `comptime count = len(Self.Ts)` can be the library `len`'s
    application over `TypeList[Self.Ts.values]()` once the checker stops
    folding `len` by name.
  - Probe: `def f[*Ts: Movable]() where len(Ts) == 2`, called as
    `f[Int, Bool]()`, prints its body in Mojito, where the pin rejects it
    for "lacking evidence" of `len[TypeList[Int, Bool]](TypeList[Int,
    Bool]()) == Int(2)`.
  - Probe: `Box[len(Ts)]` passed where `Box[Ts.length]` is expected is
    rejected by both, with the pin naming `len[TypeList[Ts.values]](Ts)`.
  - Found while landing R988 (2026-10-22).
  - Depends on R1528.
  - Model: Opus, Not Planned.

- [ ] **R594 A trait member's `Self` is respelled `_Self` in its syntax**

  The elaborator and the check see a trait default through a declaration
  whose `Self` an AST rewrite respells `_Self`, while upstream binds the
  trait's `Self` to the member's `_Self` parameter where it resolves.
  - `trait_member_declaration` (`checker/trait_defaults.rs`) runs
    `SpellSelfParam` over the default's signature and body, and respells a
    `Self.X` bound `_Self.X`.
  - The elaborator writes the respelled body back into the trait, so MIR
    and the checked tree hold `_Self` where the source says `Self`.
  - The lever: the check binds `Self` to the `_Self` parameter
    (`self_ty`, `associated_type_for_self` over a `Ty::Param`), and the
    elaborator names the member's binders `Self` and `Self.<member>`.
  - Found while landing R551 (2026-10-10).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R595 A trait member is checked as a function the check builds,
  and its wrapper reaches it through a module binding**

  The check checks a trait default as a `def` it builds from the trait
  method, receiver first, and binds it at module scope under its member
  symbol, while upstream checks the default over its trait's declaration
  and its wrapper names the trait's function directly.
  - `check_trait_members` runs `check_def` over `trait_member_declaration`,
    whose sites are `FunctionParam`/`FunctionReturn` keyed by a syntax id
    derived from the trait statement's.
  - The member's MIR declaration takes its receiver as an ordinary first
    parameter (`has_receiver: false`), which a direct call passes.
  - A conformer's wrapper calls it by name through `Checker::lookup`; an
    importer's resolves the package's module binding `Writer.write`.
  - The lever: check the member over the `TraitMethod` under its trait's
    sites (`check_method_body`), and resolve the wrapper's call through
    the trait's record.
  - Depends on R594.
  - Model: Fable, Not Planned.

- [ ] **R1770 A trait requirement's symbol, and the applicable table's
  key, are still spelled from syntax**

  A trait requirement's binder owner (`traits.rs`) and its default's
  `member_symbol` (`trait_defaults.rs`) are spelled by
  `lowered_method_name` from the AST, and the compile-time applicable
  table keys a module `def` by `lowered_def_name`; upstream names every
  declaration from its resolved signature.
  - Name a requirement from its typed `MethodSig` with
    `method_lowered_name(…, None)` and store it on the trait info, so
    `member_symbol` reads it.
  - Key an applicable entry by its declaration site rather than a symbol,
    so `ApplicableFunction::key` and `name_applicable_def` go.
  - Then delete `lowered_def_name`, `lowered_method_name`, `MethodShape`,
    `signature_from_ast`, `value_expr_raw`, and every
    `ast_raw` path left without a caller, porting `tests/symbol_test.rs`'s
    AST-key tests.
  - `symbol::value_expr_raw` spells any annotation value it does not
    recognize as `expr`, so two same-arity method overloads differing only
    in such a value share one symbol; this removes it.
  - Found while landing R600 (2026-10-31).
  - Depends on R1769.
  - Model: Fable, Not Planned.

- [ ] **R1771 An overloaded generic declaration's signature is typed twice
  to name it**

  A binder is owned by its declaration's lowered symbol (`ParamId::owner`),
  so an overloaded generic `def` types its signature once under
  `symbol::naming_owner` just to spell that symbol, then again under it;
  upstream types a signature once, its binders declaration-relative, and
  derives the mangled name afterward.
  - The readers of owner == symbol: `mir/param_uses.rs`'s owner filters,
    `native/mono/specializer.rs`'s removed-parameter keys, and the MIR
    text format's binder spelling.
  - Same direction as R1768: declaration-relative binder identity.
  - Found while landing R600 (2026-10-31).
  - Depends on R1770.
  - Model: Fable, Not Planned.

- [ ] **R561 A package body's types name the package check's binding
  owners**

  149 of the bundled library's 874 bodies carry 1,259 `origin_place`
  roots that are the package build's check-local binding owners, each at
  or past the body's variable count.
  - Mostly in register types and a subscript call's reference result
    type.
  - The MIR text spells them `$v<n>` as though they were variables, and
    the package's bytecode carries them as bare numbers under
    `origin::encoding_packaged_owners`, the one place an `OwnerId`
    encodes.
  - Upstream's types name SSA values or origin parameters, never a
    check-local binding.
  - The lever: lowering closes a type's owner roots to the variable
    `owner_vars` maps each to, or to an unbound origin; then the permit is
    deleted.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R626 A method overload's losing candidates leave their argument
  facts in the checker's stores**

  Selecting among a method's overloads scores every candidate by inferring
  its arguments into the shared fact stores, with no probe around it.
  - `score_method_call` (`method_calls/selection.rs`) is called per
    candidate from `method_calls/resolution.rs` (two loops),
    `method_calls/statics.rs`, `method_calls/type_receivers.rs`,
    `indexing.rs`, and `declarations.rs`.
  - A loser's facts at an argument stay unless the winner writes the same
    span: the last candidate scored, not the selected one, may own them.
  - Upstream's checked IR holds the chosen call's ops alone.
  - The lever: score each candidate inside `Checker::probe`, as the
    free-call overload loop does, and re-run the selected one so its facts
    alone are kept.
  - First check which of the winner's argument facts are written today only
    by its scoring, since those must come from the re-run.
  - Found while landing R612 (2026-10-11).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R627 An overload probe leaves the lifting memo, effect
  observations, and parametric write frames a losing candidate wrote**

  `Checker::probe` undoes every fact store, but a losing candidate's writes
  to the checker's other state survive it.
  - `lifted_applications`/`lifted_counts`/`lifted_displays`
    (`comptime_validation.rs`): a compile-time expression only a loser
    lifted still becomes a `$comptime$$N` function.
  - `effect_observations` and `call_through_observations`: a loser's reads
    only make a body re-infer instead of copying its facts.
  - `parametric_write_frames` (`method_calls/receiver_effects.rs`): a
    loser's mutating method call on a parametric origin counts as a write.
  - The owner-identity cursors advance: a loser's identities go unused,
    which is harmless.
  - The lever: give each a probe mark (truncate, or journal the map), and
    open it in `ProbeMarks` beside the fact stores.
  - Found while landing R612 (2026-10-11).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R640 A trait default wrapper's forwarding call is found by its
  syntax and its instantiation by span**

  `member_defaults` (`mir.rs`) matches the wrapper body's `Return`/`Expr`
  syntax for the forwarding call, then looks its
  `GenericInstantiation` up in the span-keyed
  `CheckedProgram::generic_instantiations()`.
  - Upstream's wrapper reads its member's signature; the call's
    instantiated arguments are its node's `InstantiatedArguments`
    adjustment, reachable through the wrapper's checked statement.
  - Done when `member_defaults` reads the call node and no MIR code calls
    `generic_instantiations()`.
  - Found while landing R619 (2026-10-12).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1927 An owned heterogeneous collector is the tuple of its
  elements' values, where upstream's is an owned `VariadicPack` of their
  references**

  `var *args: *Ts` binds `TyKind::VariadicPack(*Ts)` in its body, which
  `native::mono::substitute_ty` rewrites to the `Tuple` of the bound
  elements, and the call builds a `tuple.make` of the moved values, where
  the pin types it `VariadicPack[..., is_owned=True, *Ts]` over the
  addresses of slots the call moves each argument into.
  - A `read` collector is the bundled `VariadicPack` already
    (`build_variadic_pack`, `mir/variadics.rs`).
  - Lever: `variadic_collector_type`'s owned arm returns
    `variadic_pack_type(..., true)`, and `build_variadic_pack` moves each
    element into a slot it `ended`s, as an owned `VariadicList` does.
  - `VariadicPack.__deinit__`'s `unsafe_deinit_pointee` then destroys the
    elements last to first, and `pack_drops.rs::refine_pack_drops`'s
    owning-collector branch goes.
  - `Tuple.__init__(var *args)`'s `__RuntimeTuple(*args^)`
    (`stdlib/std/builtin/tuple.mojo`) must move each element out through
    the references, or call upstream's `args^.consume_elements(...)`.
  - The mono tuple arm (`substitute.rs`), the `TyKind::VariadicPack`
    collector role, and `verify_pack_spread`'s `Tuple`/`RuntimePack`
    admission go with it.
  - Probe: `owned_pack_destroyed_in_reverse.mojo` and
    `pack_constructor_spread_served.mojo` keep their output.
  - Split from R1921 (2026-11-05).
  - Depends on R1924.
  - Model: Fable, Not Planned.

- [ ] **R1929 A read collector cannot keep alive the owner of an element
  read through a call's reference result**

  A call's reference result binds through a hidden mutable `ref` slot
  (`$call_ref`, `Lower::materialize_reference_result_place`) or a pointer
  register, whose owner is reachable only through an origin, so a
  `VariadicPack` element read through it keeps no owner live by name,
  where upstream's pack carries the elements' origin.
  - Probe: `print("end", o.value().v, c.t.v, h.o.value().v)` destroys
    `h`, `o`, then `c` after the call, where the pin destroys `c`, `h`,
    `o` (`assets/ok/condition_truthiness_reads_in_place.mojo`).
  - Keeping the hidden slot alive past the call instead is rejected when
    a later argument reads the origin: `print("i", xs[0].v, len(xs))`
    over an owned `VariadicList` reports "access to 'xs' conflicts with
    live reference", since the slot is a mutable loan where the pin's
    pack element is an immutable reference.
  - Probe: `print(idn(wp)[].v)` over `var wp = Pointer(to=w)` and
    `idn` returning its pointer argument destroys `w` before the call,
    where the pin destroys it after
    (`assets/ok/call_result_pointer_deref_keeps_owner.mojo`). The element
    is a value read through the returned pointer, whose register carries
    the loan on `w`; the slot the pack binds the value to does not.
  - Lever: give a read of a reference result an immutable reborrow, or
    record the result's owner on the element, so `build_variadic_pack`
    can keep the owner itself alive.
  - Found while landing R1921 (2026-11-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1922 `get_loaded_kgen_pack` is a compiler rule, where upstream's
  `VariadicPack` declares it over `variadic_ptr_map`**

  `args.get_loaded_kgen_pack()`, which the bundled `llvm_intrinsic` calls,
  is typed by the compiler as the loaded elements' tuple (`mc_infer.rs`,
  `types::loaded_pack_type`) and lowered to a `kgen.struct.load_indirect`
  of the pack's `_value` field (`expr_method.rs`), where upstream declares
  the method over `get_as_kgen_pack()`.
  - `MirInstr::LoadIndirect` (`kgen.struct.load_indirect`) exists, and the
    elaborator expands it (`mono/spread.rs`, `expand_load_indirect`).
  - Needs the `#kgen.param.expr<variadic_ptr_map, …>` and
    `<variadic_ptrremove_map, …>` attributes in `mlir_applied_type`, as
    parameter expressions that fold once the pack is bound, and an
    `__mlir_op.\`kgen.struct.load_indirect\`` the checker types.
  - Then declare `_kgen_element_types`, `_variadic_pointer_types`,
    `_kgen_pack_with_pointer_type`, `get_as_kgen_pack`,
    `_variadic_with_pointers_removed`, `_loaded_kgen_pack_type`, and
    `get_loaded_kgen_pack` in `variadics.mojo`, and delete the rule.
  - Probe: `rg get_loaded_kgen_pack crates` finds nothing, and
    `llvm_intrinsic_shuffles.mojo` and `bit_count_ops.mojo` run on both
    backends.
  - Split from R1302 (2026-11-05).
  - Depends on R1921.
  - Model: Fable, Not Planned.

- [ ] **R1924 Upstream's `__get_litref_as_mvalue` and
  `__get_address_as_owned_value` are not builtins**

  Both are rejected as undefined, where the pin turns a `!lit.ref` into the
  place it refers to and a `!kgen.pointer` into the owned value it holds.
  - Probe: the bundled `VariadicPack.__getitem_param__` returns
    `Pointer(_mlir_value=litref_elt)[]` where upstream returns
    `__get_litref_as_mvalue(litref_elt)`.
  - Probe: `VariadicPack.consume_elements` is not declared: upstream's body
    passes `__get_address_as_owned_value(ptr._get_kgen_pointer())`, and the
    bundled `Tuple`'s `unsafe_take_pointee()` spelling is rejected for an
    element that is not trivially destructible ("requires a trivially
    destructible element").
  - Lever: type each beside `__get_mvalue_as_litref`
    (`checker/operators.rs`): the first a place through the reference, the
    second a move out of the address.
  - Found while landing R1302 (2026-11-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1681 `Pointer` is the compiler's `TyKind::Pointer`, where upstream
  declares `struct Pointer` in `std/memory/pointer.mojo` over a
  `!kgen.pointer` field**

  Its members are checker rules here (`unsafe_origin_cast`, `unsafe_mut_cast`,
  `_get_kgen_pointer`, the `_mlir_value` read, and both
  `Pointer[T, o](_mlir_value=)` constructions in
  `checker/method_calls/builtin_types.rs` and `checker/operators.rs`), where
  upstream's are Mojo bodies over its one field.
  - Probe: `p._mlir_value = k` is rejected as unsupported; the pin writes the
    field.
  - Probe: ``__mlir_op.`lit.ref.to_pointer`(r)`` and
    ``__mlir_op.`lit.ref.from_pointer`[_type=…](k)`` are not accepted.
  - Probe: the bracket spelling
    ``__mlir_type[`!lit.ref<`, T, `, `, o._mlir_origin, `, `, as, `>`]``
    needs `Origin._mlir_origin`; only `Pointer[T, o]._mlir_lit_ref` reaches
    the type.
  - Address spaces wait on it: `Pointer` and `!kgen.pointer<T>` hold only
    the default one.
  - A `ref` binding's `TyKind::Ref` and a `!lit.ref` value
    (`TyKind::LitRef`) are two types, where upstream's are one.
  - R1679's display probe is part of it.
  - The port spans sessions: this entry's session plans it and files the
    slices as entries chained by Depends bullets, replacing this one.
  - Depends on R221 (`pop.pointer.bitcast`, the body of `unsafe_bitcast`).
  - Model: Fable, Not Planned.

- [ ] **R1314 The call builds a `VariadicList`'s span itself, where
  upstream calls the list's `@implicit` constructor from the `!pop.array`**

  Problem: every call of a `*xs: T` callee stores its element pointers'
  `!pop.array` in `__passed_varargs__`, as upstream's does, then builds the
  `Span` and the list over it as two `struct.create`s
  (`mir::variadics::build_variadic_list`), where upstream calls
  `VariadicList.__init__[size, container_origin]` on a reference to that
  slot, inlined.
  - Probe: the bundled `VariadicList` declares no constructor
    (`stdlib/std/builtin/variadics.mojo`'s header).
  - Probe: the elaborator's `Array` display construction
    (`mono/substitute.rs::array_literal_construction`) shares the builder.
  - Probe: an empty list's array holds one uninitialized pointer, where
    upstream passes a null `!lit.ref` to a `!pop.array<0, …>`, which the
    constructor's gep addresses without reading.
  - Found while landing R1073 (2026-10-23); the array became a
    `!pop.array` with R1304.
  - Depends on R1315, R1316, R1317 (the constructor's vocabulary), and
    R221 (its `unsafe_bitcast` of the element pointer).
  - Model: Fable, Not Planned.

- [ ] **R1292 A MIR `try` may lack its `except` or `else` region, where
  upstream's `lit.try` always holds both**

  `MirInstr::Try` keeps `handler` and `orelse` optional, where
  `parseTryStmt` creates both regions for every `try`.
  - Upstream stubs an `except` holding `hlcf.unreachable` outside a
    raising context, and an `else` holding a lone `lit.try.yield`.
  - So the body's `try.yield` enters the `else` there, and only the
    `else`'s and handler's define the results.
  - Mojito's absent `else` makes the body's `try.yield` define the results
    instead, a special case in the VM cursor, Pliron's `lower_try`,
    `simplify-cf`'s `remove_trivial_try`, the verifier's `TryBody`, and
    `mem-2-reg`.
  - Fix: MIR lowering emits both stubs, `handler` and `orelse` stop being
    `Option`, and those special cases go.
  - Found while landing R1264 (2026-10-23).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1438 A raising call hands its callee's error back in a register,
  where upstream passes its nearest error slot as the callee's error
  argument**

  `Raises { ty, flag, error }` makes the error a result of the raising
  instruction, and its error region stores that register into the nearest
  error slot (`Flatten::error_region`), where upstream's call passes the
  slot `findNearestErrorSlot` returns as its `ByRefError` argument and the
  callee's raise writes it there.
  - Every raise already stores into its nearest slot, a `try`'s (R1293) or
    the function's (R1437); the call is the one left.
  - The call then initializes the slot on its raise path alone, which
    upstream's `addErrorRegions` exists to show `CheckLifetimes`: the
    ownership walks (`region_walk.rs`, `drops.rs`, `verify/drops.rs`) must
    see the slot initialized at the error region's entry.
  - The VM's `raise_into_caller` writes the edge's error register; it
    would write the caller's slot. Pliron copies the outcome's error area
    into the edge's storage (`close_edge`); it would copy it into the slot,
    with no ABI change.
  - `inline-parametric` then binds the callee's error slot to the call's,
    instead of taking each raise's error back out of its store
    (`raised_store`) and leaving the inlined `$error$inline<N>` slot dead.
  - Probe: `emit-mir` of `def h(x: Int) raises -> Int: return g(x)` for a
    raising `g` shows `call g` defining `raises { flag: %rA, error: %rB }`
    and its error region storing `%rB` into `$error`.
  - Found while landing R1437 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R753 MIR still declares some bindings' slots itself**

  HIR's `Cfg::bindings` lacks a nested `def`'s closure slot and a local
  `comptime` binding's slot, which MIR declares through
  `Flatten::declare_binding_var`.
  - A parameter's binding is attached lazily too, by `binding_var`'s
    spelling fallback.
  - The lever: HIR declares every binding's slot into `Cfg::bindings`,
    and `binding_var` becomes a lookup.
  - Done when `declare_binding_var` is gone and the corpus MIR is
    unchanged.
  - Found while landing R636 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R754 A type-keyed subscript's type is an index node, where
  upstream's is the accessor call's parameter attribute**

  `v[Int]` on the bundled `Variant`/`Tuple` keeps `Int` as the
  `CheckedOp::Index` index node, and MIR recognises it by its missing type.
  - Site: `type_keyed_accessor_call` (`mir.rs`), used by
    `expr_access.rs` and `lower_stmt.rs`.
  - The check already binds the index as the accessor's type argument
    (`indexing.rs`, a synthesised `ParamArg::Value`), so it records a
    `NodeFacts::argument_attribute` at the index.
  - The lever: the checked op for a type-keyed subscript carries that
    attribute, as upstream's `__getitem__[T]()` call does, and MIR decides
    by the op.
  - Done when `type_keyed_accessor_call` reads no `ty.is_none()`.
  - Found while landing R658 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R756 Value-shaped bracket arguments carry two attribute facts that
  can disagree**

  A value-shaped argument binding resolves to a type or an origin is
  recorded twice: in the call's `NodeFacts::parameter_attributes` and at
  the argument (`NodeFacts::argument_attribute`).
  - The leaf binders (`_unqualified_type_name[T]`, the
    builtin collection, tuple, pointer, and `Variant` argument types,
    `TypeList.of`) record only the first, so the two disagree there.
  - Upstream's op carries the attribute once, on the call.
  - The lever: `runtime_param_arg_expressions` (`checker.rs`) and
    `checked_parameter_arguments` (`call_contract.rs`) read the call's
    `parameter_attributes`, and `argument_attribute` is deleted.
  - Done when `NodeFacts::argument_attribute` is gone and the corpus MIR
    is unchanged.
  - Found while landing R659 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R755 The check still resolves some explicit bracket arguments more
  than once while binding**

  A few routes resolve an application's bracket list again after its
  binder already did, where upstream binds each argument once.
  - `explicit_type_subst` (`declarations.rs`) resolves the explicit type
    arguments before `resolve_use_params` resolves them again.
  - `record_static_receiver` (`method_calls/statics.rs`) binds a static
    call's receiver application again after
    `infer_struct_static_method` bound it.
  - The lever: each route keeps its binder's `BoundParameters` (or
    dimensions) and reads them where it now re-binds.
  - Done when each named site reads its route's earlier binding.
  - Found while landing R659 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R757 Bracket attributes are keyed by source position, where
  upstream's op keys them by parameter**

  `NodeFacts::parameter_attributes` and `CheckedParamArg` follow the source
  bracket list, and MIR's `param_arg_regs_folding` walks source positions,
  the folded ones named by position
  (`SemanticAdjustment::FoldedParameterArguments`).
  - Upstream's call op carries the bound parameter list in declaration
    order, with defaults filled in.
  - The lever: binding returns its attributes per declared parameter
    (`BoundParameters::arguments` already is), the checked op carries
    that list, and MIR reads it by parameter.
  - Done when no checked fact names a bracket argument by its source
    position.
  - Found while landing R659 (2026-10-13).
  - Depends on R756.
  - Model: Opus, Not Planned.

- [ ] **R758 A trait method op holds no checked signature, where
  upstream's trait declares each requirement's function type**

  `CheckedTraitMethod` holds its parameters' names, conventions, and
  defaults, but no type, result type, or compile-time parameter list.
  - The check records no declaration site for a trait method, so the
    arena builder has nothing to read.
  - `TraitRequirement` (`declaration.rs`) holds parameter types and
    conventions, but no result type, no `ParamDecl`s, and no receiver
    convention.
  - Probe: so a `binary` operator through a bound whose requirement takes
    `var self` is read as borrowing its left operand
    (`Callee::receiver_convention`, `mir/register_ends.rs`), which R1329's
    register drops would destroy a second time.
  - The lever: record a requirement's checked signature where
    `checker/traits.rs` builds its `MethodSig`, and have the trait op hold
    a `CheckedSignature`, as `CheckedMethod` does.
  - Found while landing R630 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R759 A `comptime` declaration op holds no compile-time parameter
  list, where upstream's parametric alias declares one**

  A module or local `comptime` alias, a struct's associated member, and a
  trait's `comptime` requirement name no parameters on their ops.
  - The check keeps them privately, in `ComptimeAlias::decls` and in each
    trait's member requirements.
  - `ConformanceFacts::parameterized_associated` covers struct members
    only.
  - The lever: record each declaration's `ParamDecl`s at a site and have
    `CheckedStmtOp::Comptime`, `CheckedStructComptime`, and
    `CheckedTraitComptime` hold them.
  - Found while landing R630 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R760 A package unit still encodes declaration-site rows its ops
  now hold**

  A unit's `SiteFacts` shares every `FunctionParam`, `FunctionReturn`,
  `MethodParam`, `MethodReturn`, and `StructField` type row, every
  function and method compile-time parameter list, and their effects, but
  MIR now reads all of them from the declaration ops.
  - Only a nested closure's `FunctionType` row and the struct's own
    parameter list are still read from the sites.
  - The lever: stop sharing the rows the ops hold in
    `DiscoveryResult::units`, and measure the package size.
  - A default wrapper's signature is filled from the same rows in
    `CheckedProgram::new`, after the checker built it with an empty one.
    Filling it where `checker/trait_defaults.rs` records the wrapper's
    sites would let those rows go too.
  - Found while landing R630 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R761 `ParamDecl` has no origin kind, so the check drops every
  `Origin` and `OriginSet` parameter from a declaration's parameter list,
  where upstream's list holds them in declared order**

  `classify_params` (`checker/declarations.rs`) skips each origin
  parameter and its infer-only `mut` binder, for functions and structs
  alike, so `ParamDecl` has only `Type` and `Value`.
  - The lever: add an origin `ParamDecl` kind, emitted in place, with
    slots numbered by declared position; callers wanting only type and
    value binders filter on the kind.
  - Watch for: `ParamId` slots shift for every declaration with an origin
    parameter, as do `bind_reference` and the package type section, so
    bump the package format.
  - Check whether R242 and R726 stem from the dropped binders; unverified.
  - Found while landing R631 (2026-10-13).
  - Probe: `Self.mut` in a method of `struct V[mut: Bool, //, origin:
    Origin[mut=mut]]` is rejected ("'Self.mut' is not a type parameter of
    the enclosing struct"), where the pin reads it. The bundled `Span` has
    no `write_repr_to` for it, so `repr(Span(xs))` is `Writable`'s
    synthesized default (`Span[SIMD[DType.int, 1], origin#2](_data=...,
    _size=Int(2))`) where the pin prints `Span[mut=True, SIMD[DType.int,
    1]]([Int(1), Int(2)])`.
  - The struct row's `origin_params` (MIR text 1.168) stands in for the
    origin kinds a struct's `param_decls` lacks; R762 retires it.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R762 A struct instance's origin arguments are a tail after its
  type and value arguments, numbered by source position, where upstream's
  are positional in the declared list**

  `TyKind::Struct`'s arguments put origins after the `ParamDecl`-aligned
  prefix, which `StructInfo::{origin_slots, origin_tail}`,
  `struct_origin_slots`, and `tail_origin_bindings` maintain.
  - The lever: order a struct's arguments by its declared parameter list,
    origins in place, and delete the tail machinery.
  - Sites: `checker/origins/{subst,solve,actuals,construct,exclusivity}.rs`,
    `generics.rs`, `declarations.rs`, `type_resolution.rs`; check whether
    mangling, layout, MIR verification, or mono index arguments by
    position.
  - Split into a checker half and a mono/verify half if it does not fit
    one session.
  - Probe: `It[Self.element_type, Self.origin, origin_of(self),
    Self.is_owned](0, self)` over `struct It[m: Bool, //, T: AnyType, o:
    Origin[mut=m], l: ImmOrigin, owned: Bool]` fails MIR verification
    ("value parameter 'is_owned' has no runtime register"): MIR aligns a
    call's compile-time slots positionally against `ParamDecl`s that hold
    no origins (`verify/subscripts.rs`), so the bundled
    `VariadicList.__iter__` names the iterator's origin arguments by
    keyword.
  - `StructDeclaration::origin_params` (MIR text 1.168) is the tail's
    handoff, keying each origin argument to the id a member template
    names; it goes away once origins are `ParamDecl`s in place.
  - Probe: an erased spelling (a method's `self` typed `BagIter` with no
    arguments) carries no tail, so `StructDeclaration::associated_type`
    leaves a template naming `Self.o` open there, where upstream's
    `Self` carries every parameter.
  - Found while landing R631 (2026-10-13).
  - Depends on R761.
  - Model: Fable, Not Planned.

- [ ] **R763 The checker keeps a struct's parameter syntax to validate
  origin arguments, where upstream binds them against the checked
  parameter list**

  `StructInfo::source_params` holds the struct's `TypeParam`s so direct
  applications can check explicit origin arguments, and
  `is_origin_type_param` and `type_resolution::is_origin_parameter`
  repeat one predicate over that syntax.
  - The lever: bind and validate origin arguments from the checked
    `ParamDecl`s, then delete `source_params` and the duplicate predicate.
  - Found while landing R631 (2026-10-13).
  - Depends on R762.
  - Model: Fable, Not Planned.

- [ ] **R764 The check keeps its facts in an occurrence-keyed map, not on
  nodes it allocated as it checks**

  The checker writes `NodeFacts` into `FactMap<SourceSpan, NodeFacts>`,
  where upstream's emitter sets each attribute on an op it already made.
  - The lever: a checker `Emitter` (`checker/emit.rs`) allocates a
    statement's node and its expression nodes when the check enters the
    statement, and `record_at`/`fact`/`has_fact`/`clear_fact`/`clear_flag`
    resolve an occurrence to its node through an index, or to a detached
    record where no node holds it.
  - Probes truncate the arenas and restore journaled records; a carried
    body copies its node range relocated with `CheckedOp::map_operands`
    and `CheckedStmtOp::map_operands`, and replays its index entries.
  - The handoff stays a span-keyed map read out of the nodes, so MIR text
    and package bytes must not change.
  - Plan: `R624-the-check-writes-fact-records-for-plan.md`, slice 2.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R765 Spelled constructions, lifted applications, `with` desugars,
  lambdas, trait members, and wrappers get their nodes only from the
  arena builder after the check**

  `ArenaBuilder` still decides these sites from syntax and facts, so the
  emitter's tables cannot yet replace `finalize`'s.
  - The lever: allocate each site's nodes where the check decides it,
    fill declaration ops from the checker's site tables at handoff, and
    seal `EmittedExpr` into `CheckedExpr` per node.
  - Cross-check the sealed tables against `finalize`'s under
    `MOJITO_CHECK_EMITTED=1`, per extracted root, on the probe set with
    and without `MOJITO_STD_PACKAGE=0`.
  - Plan: `R624-the-check-writes-fact-records-for-plan.md`, slice 3a.
  - Depends on R764.
  - Model: Fable, Not Planned.

- [ ] **R766 The checked handoff is still gathered from fact records by
  `finalize` and `build_checked_expressions`, where upstream's parser
  emits typed ops as it checks**

  `CheckedProgram::new` walks the syntax again after the check to build
  the nodes.
  - The lever: `HandoffStores::into_discovery` hands over the sealed
    emitted tables, and `CheckedProgram::new` takes them as they are.
  - Done when `finalize`, `build_checked_expressions`, `ArenaBuilder`,
    `CheckedArena`, and `LiftedApplications` are deleted, with MIR text
    and package bytes unchanged.
  - Update `docs/architecture.md` §Stage 3 and `docs/symbol-map.md`, and
    file what stays (detached records, `explicit_destroy` walking syntax).
  - Plan: `R624-the-check-writes-fact-records-for-plan.md`, slices 3b
    and 4.
  - Depends on R765.
  - Model: Fable, Not Planned.

- [ ] **R767 `DiscoveryResult` still carries the check's records to
  `CheckedProgram::new`**

  Once the check emits sealed tables, the handoff record is a leftover.
  - The lever: `HandoffStores` builds the `CheckedProgram` from the sealed
    tables and its declaration-level tables, and `DiscoveryResult`,
    `finalize`, and `PassCarry::into_result` go.
  - Delete the syntax half of `desugar::splice_desugars` if
    nothing but the arena builder reads the spliced statements.
  - Renaming `DiscoveryResult` first (to `CheckedFacts`, as its doc
    describes it) is moot: the type goes here.
  - Update `docs/pliron-future.md`'s "checker (+ checked handoff)" row to
    say the output is emitted ops.
  - Then judge whether the node tree's remaining distance from `lit`'s
    ops in regions earns its own entry, and file it if so; the plan
    expects the distance to be recorded in that row instead.
  - Plan: `R625-discoveryresult-still-stands-between-the-check-plan.md`,
    Entry A.
  - Depends on R766.
  - Model: Fable, Not Planned.

- [ ] **R768 `CheckedDeclaration` repeats what the statement nodes hold,
  where upstream's symbol is its op**

  Each declaration's kind, name, binding, and type are its statement
  node's, and only HIR's `for` binding and MIR's nested `def`s read it.
  - The lever: those two readers read the node, and
    `CheckedDeclaration`, `CheckedDeclKind`, `CheckedStmt::declaration`,
    `CheckedTables::declarations`, and `CheckedTables::declare` go.
  - A nested `def` sharing its name with a sibling is named by its
    position among same-named siblings, not by the global
    `CheckedDeclId`, which a package unit renumbers: a library function
    with two same-named nested `def`s would be refused by
    `refuse_unequal_units` (argued from the code, not reproduced).
  - The package format moves.
  - Found while landing R625 (2026-10-13).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R611 A package unit's facts name the package check's binding
  owners**

  A unit's binding facts carry the build's check-local `OwnerId`, where
  an upstream operand names the value its declaration defines.
  - The facts are an expression's or statement's binding, a capture's,
    and a comprehension binder's; they encode as bare numbers under
    `origin::encoding_packaged_owners`, beside R561's owners in MIR
    register types.
  - Renumbering them per unit would break their match with the origin
    roots inside the facts' types, which the package's type section
    holds once for every unit.
  - The lever: once a type's owner roots name no check-local owner
    (R561's lever, extended to the checked facts' types), encode a
    binding as its declaring occurrence and draw one fresh owner per
    declaration on decode; then the permit goes.
  - Found while landing R604's first slices (2026-10-10).
  - Depends on R561.
  - Model: Fable, Not Planned.

- [ ] **R609 A type's identity below the check is its structure, not its
  storage**

  Mono, HIR, MIR lowering, and the MIR text reader build types without the
  compilation's context, so two equal types there may be distinct nodes and
  equality keeps a structural fallback where MLIR compares pointers.
  - Only the checker's recorded types are uniqued
    (`ParamContext::intern_ty`, at `body_carry`'s handoff); a type built
    later is a fresh node.
  - `ParamExpr`'s detached contexts (`ParamContext::detached()`, about 70
    sites below the check) have the same gap.
  - The lever: one compilation context reaches every phase that builds a
    type, carried by `CheckedProgram` across the waist, and `Ty::new`
    takes it.
  - Found while landing R585 (2026-10-10).
  - Depends on R610.
  - Model: Fable, Not Planned.

- [ ] **R610 A type carries decorations its identity leaves out**

  A callable type's transfer effects and a dictionary or set value
  argument's display spelling ride inside a `Ty` but take no part in its
  equality or hash, so a uniquing table must refuse such a type.
  - `Ty::carries_decorations` reports them, and `ParamContext::intern_ty`
    and `ParamContext::intern` decline to share such a node.
  - Upstream's uniqued types carry no payload their identity ignores.
  - The lever: transfer effects move to the callable value's checked facts,
    and the display spelling to the display's own record.
  - Found while landing R585 (2026-10-10).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R586 A register's type is a hash-map entry, not a register-indexed slot**

  `MirFunction::reg_types` and `var_tys` (`crates/mojito-mir/src/mir/ir.rs`)
  are hash maps keyed by register and variable number, while MLIR attaches
  a value's type to the value itself.
  - Decoding the bundled bodies' `reg_types` maps is 21% of the hot body
    decode (1.6 of 7.7 ms, measured outside the repository by decoding
    each field separately).
  - A `Vec<Option<Ty>>` sized by `n_regs` is the value-attached shape.
  - Every MIR consumer reads the maps, so the change is its own task.
  - Found while landing R584 (2026-10-10).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R589 A method's reference contracts are held as checker `RefSig`s
  beside MIR's coarser masks**

  A method's signature keeps each `ref` parameter's origin contract and
  mutability as a `RefSig` (`MethodSig::ref_params`, `ref_return`), while
  its MIR declaration holds `ref_params` and `param_writes` bool masks.
  - `ref_params` restates `param_conventions`.
  - `param_writes` derives from the conventions plus the mutability a
    `RefSig` decides.
  - The package keeps both: the residue's `RefSig`s and the declaration's
    masks.
  - Upstream's parameter type holds the reference contract, and nothing
    restates it.
  - The lever: the declaration holds each contract with its parameter, and
    the masks and the residue's `ref_params`/`ref_return` derive from it.
  - Related: R581 drops the mask the directory row restates.
  - Found while landing R574 (2026-10-10).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R769 A call through a bound runs a requirement default by
  rewriting its syntax**

  The check completes such a call by spelling the requirement's default
  as keyword syntax and checking the program again, while upstream
  materializes the callee signature's default attribute at the call.
  - The rewrite is `bind_bound_default_arguments`
    (`checker/bound_defaults.rs`) and the extra pass in
    `check_program_carrying`.
  - The signature already holds the attribute (`SignatureDefault`); a
    non-constant parameter value is spelled back from its `ParamExpr`
    (`spelled_parameter_value`) only to feed the rewrite.
  - The lever: the checked call node records each omitted slot's
    requirement default, evaluated at the call, and HIR/MIR lower it as an
    argument (a constant, or a parameter value).
  - A requirement default must then be typed against its parameter at the
    trait, so its conversion (`None` into `Optional`) is known there.
  - This spans the checked call node, HIR, MIR, the VM, and Pliron: the
    planning session should split it into chained entries.
  - Probe: `go(a)` in `assets/ok/evaluated_default_argument.mojo`, over
    `trait Shows: def show(self, x: Optional[Int] = None)`: the
    requirement's `None` is still the conformer's to bind, since a bodiless
    requirement has no lowered declaration for `mir::fill_omitted_arguments`
    to read, where a trait default's body is filled through its dispatch
    symbol (`u.f()` in `assets/ok/trait_default_parameter_defaults.mojo`).
  - Found while landing R590 (2026-10-13).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R788 A local compile-time binding of a non-application parameter expression still stores a slot**

  `comptime m = N + 1`, a folded `comptime n = 5`, `comptime m = f(1) + g(2)`, and an evaluated value (`comptime t = (L[0], L[1])`) each store one slot at the declaration, where Mojo's alias materializes every read.
  - The difference cannot be observed for these types (`Int`, `Bool`, trivially destroyed values), so this is structure only.
  - A binding of an application, or of a field chain of one, already has no slot: `Checker::local_comptime_application` makes each read a `ParamValue`, and `CheckedStmtOp::Comptime::denotes` tells MIR.
  - The fix widens that rule to every parameter expression the binding denotes, and gives `bind_evaluated` values a per-read evaluation.
  - A binding inside a lifted thunk (`EnclosingBinders::lifted`) also keeps a dead slot today; drop it in the same change.
  - Found while landing R523 (2026-10-13).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R900 A reflected field-name list read at an `Array` is built by
  the elaborator's list-literal construction**

  Problem: `materialize[names]()` over `comptime names =
  reflect[T].field_names()` runs `Array`'s list-literal initializer at each
  read, where the pin reads the `Array` its `field_names()` built at compile
  time.
  - The elements are `String`s, so no difference can be observed.
  - Upstream's `field_names()` is a stdlib function returning
    `Array[StaticString, count]`; once it is bundled, its compile-time value
    is a CTFE application, served as display constants are.
  - The fix deletes `array_display_construction`, `ConstructionDecls`, and
    `constructed_parameter_constant` (`mono/substitute.rs`).
  - Found while landing R789 (2026-10-14).
  - Depends on R324.
  - Model: Fable, Not Planned.

- [ ] **R306 A scalar `range(...)` is typed by a checker rule where the
  pin declares infer-only overloads**

  `range(Int32(4))` is typed by `Checker::infer_scalar_range`
  (`checker/builtins.rs`) once overload selection fails, which spells the
  call as the range struct's construction, where the pin's
  `std.builtin.range` declares `def range[dtype: DType, //](end:
  Scalar[dtype]) -> _ZeroStartingRange[dtype]` and its two- and
  three-argument siblings.
  - Nothing blocks the source overloads now: `def range[dtype: DType,
    //](end: Scalar[dtype])` added to `stdlib/std/range.mojo` beside the
    `Int` one is selected for `range(Int32(3))`, `range(3)` keeps the
    `Int` overload, and its `comptime assert` fails a float lane at
    elaboration (2026-10-26).
  - The pin's float range is `_StridedRange[dtype, forward]` under
    `comptime if Self.dtype.is_floating_point()`; Mojito's is the separate
    `_FloatStridedRange`. Fold it into `_StridedRange`, and assert the
    pin's "a floating-point range requires an explicit step" text in the
    one- and two-argument overloads.
  - Then `infer_scalar_range`, its call in `Checker::infer_call`
    (`call_inference.rs`), and the `scalar_range_parts` shortcuts in
    `iteration.rs`, `indexing.rs`, and `builtins.rs` go.
  - Probe: `range(Scalar[dt](0), n)` over `n: Scalar[dt]` in `def f[dt:
    DType]` is "no overload matches the supplied arguments", where the pin
    prints `0`, `1`, and `2` for `f[DType.int32](3)`: the rule reads only
    a concrete lane, and `range(Int32(0), Int32(3))` runs.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R904 A `comptime for` over a type pack or a reflected field-type
  list fails with Mojito's own wording**

  Problem: the pin rejects both loops because a `TypeList` has no
  `__iter__`, while Mojito fails somewhere else with its own message.
  - `comptime for T in reflect[P].field_types()` gives "not a compile-time
    Int constant: 'reflect[P].field_types()' is a compile-time list"; the
    pin says "'TypeList[Int, String]' does not implement the '__iter__'
    method".
  - `comptime for T in Ts` over `*Ts: AnyType` gives "'TypeList[AnyType,
    Ts]' does not implement the '__iter__' method"; the pin says
    "'TypeList[Ts.values]' …". Only the type's display differs, which is
    R1489.
  - Add a `type_error` fixture for each once the check gives the pin's
    wording.
  - Found while landing R795 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R798 The bundled `paramfor_*` stubs bound their iterator by
  `Deinitable` as well**

  Problem: `stdlib/std/builtin/_stubs.mojo` bounds `IteratorType` by
  `Iterator & Copyable & Deinitable`, where upstream's bound is `Iterator &
  Copyable`.
  - The extra bound stands in for upstream's `trait Iterator(Deinitable,
    Movable)`, which the bundled `Iterator` does not refine yet (R669).
  - Drop `& Deinitable` from the three stubs once it does.
  - Depends on R669.
  - Model: Opus, Not Planned.

- [ ] **R799 The bundled `paramfor_*` stubs trap through `_mojito_abort`**

  Problem: `paramfor_next_iter` and `paramfor_next_value` in
  `stdlib/std/builtin/_stubs.mojo` end their `except` arm in
  `_mojito_abort(...)`, where upstream calls `_abort_base()`, a `-> Never`
  function.
  - A call of a `-> Never` function does not end a path for return
    analysis yet (R50).
  - Swap in `_abort_base()` and give `std.os` its declaration once it does.
  - Depends on R50.
  - Model: Opus, Not Planned.

- [ ] **R894 Bundled modules loaded while the prelude bootstraps do not see
  the prelude's names**

  Upstream's library modules use prelude names such as `range` without
  importing them. Mojito's prelude dependencies see only their explicit
  imports.
  - `Linker::load_module` (`crates/mojito-module/src/module.rs`) gives a
    module no implicit bindings while `prelude_bindings` is still `None`.
  - Without an import, such a module binds the checker's builtin `range`,
    which has no `__iter__`, so its `comptime for` cannot step an iterator.
  - Workaround: `builtin/tuple.mojo`, `utils/variant.mojo`, `string.mojo`,
    `format/__init__.mojo`, `format/_utils.mojo`, and `format/tstring.mojo`
    import `range` from `std.range` explicitly.
  - Probe: `builtin/builtin_slice.mojo` (`Slice.indices`) and
    `os/path/path.mojo` call `max` without an import, so they reach the
    checker's builtin `min`/`max` rule (`Checker::infer_min_max`), the VM's
    `builtin_min_max`, and Pliron's `lower_min_max_builtin`, where every
    other module calls `std.math`'s library overloads. `simd.mojo` imports
    `max` and `min` from `std.math` for `clamp`, and
    `builtin/format_int.mojo` imports `abs` and `max`.
  - Probe: `simd.mojo` imports `Optional` and `StaticString`, and
    `builtin/format_int.mojo` and `builtin/_format_float.mojo` import
    `StaticString`, `StringSlice`, and `Span`, which upstream's read from
    the prelude: without them `std.simd`'s import of the two fails the
    package build ("not a compile-time Int constant: StaticString",
    "no overload matches" `write_string`).
  - Fix: bind the prelude's names in bootstrap modules too, then drop the
    six `range` imports, `simd.mojo`'s `max`/`min` import, and
    `format_int.mojo`'s `abs` and `max` imports, and delete the `min`/`max` and `abs`
    rules (`Checker::infer_min_max`, `infer_abs`), their
    `call_inference.rs` arms, the VM's `"min"`/`"max"`/`"abs"`
    `BUILTIN_CALLEES` entries and arms, and Pliron's
    `lower_min_max_builtin` and `lower_abs_builtin` with their `calls.rs`
    arms. `abs`'s elaborator rewrite (`mono/specializer.rs`) goes with it.
  - Found while landing R791 (2026-10-14).
  - `from std.math import max` in `builtin/builtin_slice.mojo` and
    `os/path/path.mojo` packages, so `std.math` links ahead of them.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R492 `ParameterList.get_span` addresses its elements through a
  compiler-private stand-in**

  Problem: the bundled `ParameterList` takes its elements' static address
  from `__param_list_address[*Self.values]()` (`MirInstr::ParamListAddress`),
  where upstream's body is `global_constant` over the list's
  `#pop.variadic_to_array` and an `unsafe_bitcast` of the array's address.
  - Porting upstream's body needs `std.builtin.globals` and an inline,
    trivially copyable `Array`; the bundled `Array` keeps its elements
    behind an `UnsafePointer`.
  - The stand-in is admitted only in `std/builtin/variadics.mojo`.
  - Found while landing R325 (2026-10-08).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R800 A compile-time subscript of a value pack is a checker rule, not
  `ParameterList.__getitem_param__`**

  `comptime e = vs[1]`, `Box[vs[0]]`, and `materialize[vs[i]]()` are
  compiled by the checker straight to a `ListGet` over the pack, where
  upstream reads them through `ParameterList.__getitem_param__`.
  - The checker site is the `Index` arm of `compile_dependent_ct_expr`
    (`checker/constraints.rs`), which also serves display lists and module
    display constants and must keep doing so.
  - Its value-pack arm must claim the pack ahead of the struct-value
    subscript: since R1910 a bare `vs` builds to its visible
    `ParameterList` binder, a compile-time struct value whose
    `__getitem_param__` the bundled port lacks (`comptime First[*vs: Int]
    = vs[0]`, `assets/ok/generic_alias_type_pack.mojo`).
  - Add upstream's `comptime __getitem_param__[idx: SIMDLength]:
    Self.type` to `ParameterList` in `stdlib/std/builtin/variadics.mojo`,
    the bracketed `#kgen.param_list.get` body over `idx._mlir_value` that
    `TypeList`'s already is.
  - Probe: `comptime e = vs.__getitem_param__[1]` in `def go[*vs: Int]()`
    reports "type 'ParameterList[Int, vs]' has no field
    '__getitem_param__'", where the pin prints `5`.
  - Probe: over `struct R[t: AnyType, //, values:
    _MLIR.KGENParamListType[t]]` (`_MLIR` imported from
    `std.builtin.variadics`), `comptime first: Int = Self.values[0]`
    prints `4` for `R[vs.values]` with `vs = 4, 5`, where the pin rejects
    it with "'KGENParamList[t]' is not subscriptable". The same arm must
    take a raw list only under the bundled gate (found while landing
    R1168, 2026-10-20).
  - Found while landing R494 (2026-10-13).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1489 A pack's list type displays its infer-only parameter and
  the bare pack**

  Mojito spells a pack's list type `TypeList[Movable, Ts]` and
  `ParameterList[Int, vs]`, where the pin writes `TypeList[Ts.values]`
  and `ParameterList[vs.values]`, and a closed list `TypeList[Int, Bool]`
  where Mojito writes `TypeList[AnyType, (Int, Bool)]`.
  - The pin prints a struct's infer-only parameters (`Trait`, `T`) only
    when they are not inferable from the rest, and a raw list argument as
    the pack's `.values` or its elements.
  - Probe: `comptime y: Int = Ts` and `var y: Int = Ts` in `def go[*Ts:
    Movable]` report "expected Int, found TypeList[Movable, Ts]", where
    the pin reports "cannot implicitly convert 'TypeList[Ts.values]' value
    to 'Int'".
  - Probe: `var t: Int = TypeList.of[Trait=AnyType, Int, Bool]()` reports
    "found TypeList[AnyType, (Int, Bool)]"; the pin says
    'TypeList[Int, Bool]'.
  - Probe: `var t: Int = vs` in `def go[*vs: Int]` reports "found
    ParameterList[Int, vs.values]"; the pin says
    'ParameterList[vs.values]', the infer-only `Int` elided.
  - Probe: R904's pack loop.
  - Probe: a `Tuple[*Ts]` local's type is `Tuple[*Ts.values]` at the pin
    and `Tuple[*Ts]` here, and `*args: *Ts` is `VariadicPack[False,
    *Ts.values]` there. A type pack's list binder (`Ts.values`, R1492) is
    spelled as its pack, `Ts` and `*Ts` (`ParamDecl::binder`,
    `ParamDecl::list_reference`), so the `TypeList` probe above reads
    `TypeList[Movable, Ts]` where the pin's `Ts.values` is the list
    binder's own name.
  - Found while landing R1169 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1182 `TypeList`'s list vocabulary is answered by spelling, not by
  the bundled struct's members**

  Problem: `reverse`, `slice`, `map`, `filter_idx`, `reduce`, `any`,
  `all`, `all_conforms_to`, and `contains` are compiler rules keyed on the
  name `TypeList`, where upstream declares each on the struct.
  - The checker answers them in `Checker::typelist_receiver` and
    `compile_typelist_proposition` (`checker/constraints.rs`).
  - The pre-check elaborator folds them by spelling in
    `comptime/rewrite.rs:fold_pack_typelist_use`.
  - Upstream builds each from `tabulate` or `reduce` with a generator, or
    as an alias (`reverse = TypeList.tabulate[...]`).
  - Port them to `stdlib/std/builtin/variadics.mojo` and delete the rules.
  - Found while landing R1168 (2026-10-20).
  - Depends on R1181 (`tabulate`), R806 (an alias read through the
    struct), and R1178 (a member applying `__getitem_param__`).
  - Model: Fable, Planned.

- [ ] **R378 A function lifted for a compile-time expression evaluates
  again each local `comptime` binding it reads**

  Problem: `comptime L = [n, n * 2]` read by `comptime if L[0] == 3:` and by
  `range(len(L))` in `def f[n: Int]()` is evaluated once for each of the
  two, where the pin evaluates a `comptime` alias once per instance and
  every read names that value.
  - A thunk begins with the statement of each local `comptime` binding it
    reads that denotes no parameter expression (`thunk_prologue`, `mir.rs`):
    a display, and any value that is not arithmetic over the binders
    (`comptime b = n > 2`, `comptime t = (L[0], L[1])`).
  - `materialize[L]()`, and a value read off a display that is no `Int` or
    `Bool`, build the display again where they cross
    (`Flatten::build_display`).
  - The bindings served are pure, so the values agree with the pin's and
    only the work is repeated.
  - Bind each such binding's one application in the instance, and read an
    element of it as a parameter expression over that value.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1471 MIR builds a bracket argument's register from its node
  beside the expression the check compiled**

  `flag[n + 1]()` in `def f[n: Int]()` carries the check's parameter
  expression as its instantiated argument, but MIR builds the argument's
  register again from the argument's checked node, where upstream reads
  sema's one `kgen.param.constant`.
  - The check records what it compiled only for an expression that applies
    a function (`Checker::record_compiled_expression`,
    `checker/comptime_validation.rs`).
  - Arithmetic over binders, and a `comptime for` variable's read, are
    rebuilt by `Flatten::value_binder_node` (`mir.rs`) through the same
    `ParamContext` builder, so the two agree today.
  - Lever: record every compiled bracket argument, local `comptime` value,
    and crossing operand, and let `value_binder_node` read records only.
  - Probe: `flag[n + 1]()` and `comptime k = n * 2` read by `print(k)` keep
    their output once `value_binder_node` builds nothing.
  - Depends on nothing.
  - Model: Opus, Not Planned.

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
    writes `0.10000000149011612`. The sixteen-lane `float32` `reduce_mul` in
    `assets/ok/simd_reductions_float.mojo` is one such row: Mojito prints
    `68104130396160.0`, the pin `68104130000000.0`.
  - The lever: port upstream's `std/builtin/_format_float.mojo`, which is
    already Mojo, in place of the bundled module's staged `_write_float`
    leaf, and delete the `_mojito_float_text` builtin with its checker,
    VM, and Pliron arms.
  - Its `float64` cache is `UInt128` tables, which wait on R65.
  - Probe: `Float64(-2.25).write_to(b)` into a writer that marks each
    `write_string` with `|` and wraps each `write` in `<`/`>` gives
    `|-2.25`, where the pin's Dragonbox writes
    `<|-><<|>|2><|.><<|>|2><<|>|5>`.
  - Depends on R65.
  - Model: Opus, Planned.

- [ ] **R2008 No builtin body of the bundled `struct DType` folds in the
  checker**

  Every `@always_inline("builtin")` body of `stdlib/std/builtin/dtype.mojo`
  stays unfolded in `fold_builtin_bodies` (`checker/builtin_fold.rs`), so a
  compile-time `DType` predicate leans on the hand-written
  `ParamContext::dtype_predicate` where upstream's
  `foldInlineBuiltinFunction` folds the declared body.
  - The unfolded bodies: `__init__`, `_from_ui8`, `_as_ui8`, `__eq__`,
    `__ne__`, both `_match` overloads, every `is_*`, and
    `_is_non_index_integral`.
  - Cause: `declared_fields` reads `var _mlir_value: Self._mlir_type`
    before the struct's own `comptime _mlir_type` registers, so
    `bind_struct_slots` and `constructed_fields` return `None` for every
    member. Register the parameterless type-valued `comptime` members a
    struct's field types read before bodies fold, as
    `register_builtin_result_members` does for `Self.X` results.
  - Check whether that registration also closes R2007 (the same
    `Self._mlir_type` read, at the initializer's parameter). If it does,
    restore upstream's spelling of the keyword `__init__` and delete R2007.
  - Cause: `ApplicableShape::applicable` (`comptime_validation.rs`) admits
    no `` __mlir_type.`!kgen.scalar<dt>` `` slot or result, so `_as_ui8`
    and `_match(self, mask: !kgen.scalar<ui8>)` are not applicable.
  - Cause: the fold reads none of the ops those bodies spell. `build_ct`
    has no `pop.dtype.to_ui8` or `pop.dtype.from_ui8` arm.
    `ParamOp::DtypeToUi8` is typed `SIMD[uint8, 1]` where upstream's op
    yields the builtin `ui8`. `mlir_op_operator` refuses the `ui8` ↔
    `!kgen.scalar<ui8>` builtin casts. `pop.simd.and` over
    `!kgen.scalar` storage is untested.
  - Check that `self == DType.uint` (in `is_integral`, `is_unsigned`)
    folds through the folded `__eq__` while bodies fold.
  - Probe: every `DType` builtin has a `folded_body`, and
    `is_floating_point` over a binder builds to the `_match` mask
    expression.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R2009 A `where` clause over a struct's builtin predicate is
  "unsupported generic where proposition"**

  A user struct over `!kgen.dtype` whose `@always_inline("builtin")` `is_fp`
  gates `def f[d: DT where d.is_fp()]()` is rejected, where the pin folds
  the call. Only the bundled `DType`'s predicates pass, through the
  hand-written `ParamContext::dtype_predicate`.
  - Cause: `build_ct` never folds a builtin method call. Only
    `constraint_operand` and `struct_operator_value` reach
    `called_application`, so `compile_generic_constraint`'s catch-all
    sees no expression for the call.
  - Lever: `build_ct` folds a call of a builtin with a folded body through
    `called_application`, as upstream's parser folds an inline-builtin call
    where it builds it.
  - Then delete `dtype_predicate_value` (`constraints.rs`),
    `ParamContext::dtype_predicate`, `DTYPE_PREDICATES`, and
    `dtype_predicate_folds_on_replacement`, and fix `ParamOp::DtypeToUi8`'s
    doc.
  - The folded display of a failed clause may change: re-check
    `assets/type_error/where_dtype_predicate_lacks_evidence.mojo`.
  - Probe: `rg "dtype_predicate|DTYPE_PREDICATES"` is empty, the std
    package builds, `assets/ok/where_dtype_predicate.mojo` runs, and
    `comptime if DType.float32.is_floating_point()` still folds.
  - Depends on R2008.
  - Model: Opus, Not Planned.

- [ ] **R1972 `TyKind::as_non_simd_struct` excludes a `StringLiteral[v]`
  application, so the struct rules never see it**

  `as_non_simd_struct` (`mojito-types/src/types.rs`) answers `None` for a
  string literal, where upstream's literal is an ordinary struct.
  - Probe: with the exclusion gone, `assets/ok/comptime_ctfe.mojo` fails
    ("untyped variable slot $0 ('NAME')" for `comptime NAME = "buf" +
    "fer"`), and `assets/ok/module_constant_literal_bracket.mojo` fails
    (`operator '+' over a struct value applies no
    '@always_inline("builtin")' operator method`): the parameter domain
    cannot fold `StringLiteral.__add__`'s `return {}`, which the checker
    builds by rule.
  - Probe: `assets/ok/string_literal_slice_members.mojo` then loses
    `String.__init__$ov$StringLiteral$V$value$data`'s `value` binder to
    `_REMOVED_ARG` if `member_table_name` answers the literal's bare
    `"StringLiteral"` from its `as_non_simd_struct` arm: the literal's
    per-text instance arm has to stay ahead of it.
  - The same exclusion keeps a `ty.is_string_literal()` arm in
    `trivially_register_passable_leaf` (`mojito-types/src/types.rs`):
    `MirDeclarations::copy_is_identity`, the checker's `memory_only`, CSE's
    `built_in`, `call_results`, and the synthesized copy judge a literal
    without reaching its declared `TrivialRegisterPassable`. Delete the arm
    with the exclusion.
  - Lever: the builtin fold folds `return {}` at the declared result type
    `StringLiteral[#pop.string_concat<…>]`, as upstream's does.
  - Do not re-add a literal check at one of the call sites without naming
    the upstream rule it encodes.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1973 `StringLiteral[v]()` is built by a compiler rule, not the
  declared `__init__`**

  The checker constructs a string literal by rule (`literal_construction`
  and `infer_instance_construction`, `checker/call_inference.rs`), and
  MIR's `default_construction` writes the text (`type_construction.rs`),
  where upstream runs the struct's empty `__init__(out self)`.
  - Lever: delete the string halves of both checker rules and the MIR arm,
    so `Self()`, `StringLiteral[v]()`, and `{}` reach the declared
    `__init__` and `Self(copy=x)` the synthesized copy. The numeric halves
    stay for R1968.
  - Probe: `comptime template = StaticString(Self())` in
    `StringLiteral.format` fails to package the bundled library ("Undefined
    variable '__module$std$string$_FormatUtils'"; rebuild after the stdlib
    edit, as the package is cached per build), so the bundled body spells
    `StringLiteral[Self.value]()`.
  - Probe: `comptime w = Self()` in a user struct's method runs.
  - Done when the rule is gone and the bundled `format` spells
    `StaticString(Self())`.
  - Depends on R1972.
  - Model: Fable, Not Planned.

- [ ] **R1959 A literal reaches an implicit constructor's parameter
  through a second conversion, where upstream converts once**

  `var o: Optional[Float32] = 0.5` runs, where the pin reports "cannot
  implicitly convert 'FloatLiteral[0.5]' value to 'Optional[Float32]'".
  - `Checker::implicit_conversion_constructor` (`checker.rs`) admits a
    constructor whose parameter the source `coerces` to, and `coerces`
    takes a literal into any `SIMD` lane, which upstream does through
    `SIMD`'s own `@implicit` initializer.
  - Upstream converts once: a literal reaches `Optional[T]`'s `@implicit
    __init__(T)` only as the type it materializes to (`Int`, `Float64`).
  - Lever: test the parameter against the source and its materialization
    target only, not the literal's lane coercion.
  - Probe: `var o: Optional[Float32] = 0.5 if c else None` is accepted at
    the checker, where the pin's merge reports "FloatLiteral[0.5] is not
    compatible with None".
  - Found while landing R1845 (2026-11-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1975 Two existential `Some[Trait]` parameters of one trait share
  one binder**

  `synthetic_binder` makes the spelling `Some[Trait]` a binder's whole
  identity, where upstream desugars each `Some[Trait]` occurrence to its
  own anonymous `[T: Trait]` binder.
  - So a call binding the two at different types elaborates one instance
    keyed by the first argument, and the specialized MIR fails to verify.
  - Lever: mint one binder per parameter slot when resolving a
    signature (as `with_existential_binders` in
    `checker/traits_support.rs` already numbers them for witness
    comparison), and give the body's parameter binding the same id.
  - Probe: `two(String("a"), 1)` over `def two(a: Some[Writable], b:
    Some[Writable]): print(a, b)` fails elaboration ("argument 1 of
    'two$mono$TString$TString' has type Int, declared String"), where the
    pin prints `a 1`.
  - Found while landing R1865 (2026-11-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1866 The bundled string iterators are not upstream's
  `CodepointSliceIter`, `CodepointsIter`, and `GraphemeSliceIter`**

  Upstream's `std/collections/string/iterators.mojo` declares
  `CodepointSliceIter[origin, forward]`, `CodepointsIter[origin]`, and
  `GraphemeSliceIter[origin, forward]`, where `stdlib/std/string.mojo`
  declares the private `_CodepointSliceIter`, `_CodepointSliceReversedIter`,
  `_CodepointIter`, `_GraphemeIter`, and `_GraphemeReversedIter`, one
  struct per direction.
  - So `StringLiteral`'s `__iter__`, `__reversed__`, codepoint, and
    grapheme members, and `StringSpan`'s and `String`'s, name the private
    structs in their result types, and a user cannot spell
    `GraphemeSliceIter[ImmStaticOrigin, False]`.
  - Lever: port upstream's module at its own path, with `forward` as a
    `Bool` parameter, and re-export it where upstream does.
  - Found while landing R1655 (2026-11-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2035 An LLVM intrinsic over a pointer operand is rejected, where
  the pin calls it**

  A reached `llvm.memcpy`, `llvm.memmove`, or `llvm.prefetch` call fails
  its instance ("the LLVM intrinsic '…' is outside the subset"), where the
  pin copies the bytes or prefetches.
  - Cause: `LlvmIntrinsic::signature_error` (`mojito_types::llvm_intrinsic`)
    types only `SIMD` lanes and a `Bool`, so no operand may be an
    `UnsafePointer` (LLVM's `ptr`).
  - The VM's `runtime::llvm_intrinsic` reads only lanes. A pointer operand
    needs a rule over the VM's typed memory, as `pop.pointer.*` has.
  - `FnLowering::lower_llvm_intrinsic` (`mojito-pliron`, `lower/simd.rs`)
    lowers only lane operands too.
  - Probe: `std.memory`'s `memcpy` at compile time calls
    `llvm.memcpy(dest, src, n.__mlir_index__())`, and `memmove` calls
    `llvm.memmove(dest, src, n, False)`, both over `Byte` pointers.
  - Probe: `std.sys.intrinsics`' `prefetch`, unported with its
    `PrefetchOptions`, calls `llvm.prefetch(addr, rw, locality, cache)`,
    three `immarg` `Int32`s after the pointer.
  - Found while landing R1521 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2037 The masked memory intrinsics are rejected, where the pin calls
  them**

  `llvm.masked.load`, `masked.store`, `masked.compressstore`,
  `masked.gather`, and `masked.scatter` fail their instance, so
  `std.sys.intrinsics`' `masked_load`, `masked_store`, `compressed_store`,
  `gather`, and `scatter` stay unported.
  - Each reads or writes lanes through pointers under a `bool` lane mask,
    with a pass-through vector for the lanes a load leaves off.
  - `gather` and `scatter` pass their base pointers as a
    `!kgen.simd<size, address>` vector of addresses
    (`TyKind::KgenAddressSimd`), loaded through R221's `unsafe_bitcast` of
    a `SIMD[.int, size]`.
  - Probe: upstream's `masked_load[4](ptr, mask, passthrough)` and
    `gather(base, mask, passthrough)` over a `float32` buffer.
  - Depends on R2035 (a pointer operand) and R221 (`unsafe_bitcast`, for
    `gather` and `scatter`).
  - Model: Opus, Not Planned.

- [ ] **R2036 The remaining target-independent LLVM intrinsics the pin
  calls have no lane rule**

  `llvm.exp10`, `ldexp`, `frexp`, `powi`, `scmp`, `ucmp`, the
  `*.with.overflow` family, and `readcyclecounter`/`readsteadycounter`
  fail their instance, where the pin calls them.
  - Cause: the VM computes each intrinsic itself, so each needs a lane rule
    in `mojito_types::llvm_intrinsic` before either backend admits it.
  - `*.with.overflow` and `frexp` give LLVM's literal struct of a lane and
    a flag or exponent, `{iN, i1}`, where `vector.deinterleave2`'s is two
    equal vectors.
  - `exp10` has no Rust `std` function, so its float lanes need the C
    library's `exp10`, as LLVM's lowering calls it.
  - The counters read the clock, so the VM computes a monotonic count.
  - Probe: `llvm_intrinsic["llvm.readcyclecounter", UInt64]()`, and the
    HIP plugin's `llvm.readsteadycounter` read.
  - Probe: `llvm_intrinsic["llvm.sadd.with.overflow",
    _RegisterPackType[Int64, Bool]](a, b)`.
  - Target- and GPU-specific intrinsics (`llvm.x86.*`, `llvm.nvvm.*`,
    `llvm.amdgcn.*`) are not this entry's: the VM has no target to read.
  - Found while landing R1521 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1984 A one-field struct declaring `RegisterPassable` with a
  lifecycle member is boxed in both backends, where the pin holds it as
  its field**

  The pin passes `R(Movable, RegisterPassable){v: Int}` with a
  `__deinit__` as `i64`, as it does a trivial one, but
  `MirDeclarations::register_field` still answers only a struct declaring
  `TrivialRegisterPassable` with no lifecycle member.
  - The fact is in place: `StructDeclaration::register_passable` records
    the template's unconditional `RegisterPassable`/`TrivialRegisterPassable`
    row, which every instance keeps (R1524).
  - The VM is ready: its drops, copies, and moves run by static type and
    box a held value (`dispatch::{holds_unboxed, static_struct_value}`).
    With `register_field` reading `register_passable`, the VM prints the
    pin's output for the probes below.
  - Cause: Pliron's lifecycle call sites assume aggregate storage.
    `lower_drop_var`/`lower_drop_reg` (`lower/drops.rs`) return early for
    `LowerTy::Scalar`, `emit_drop` passes the value's address,
    `emit_constructor_call` (`lower/pointers.rs`) passes `(dst, src)`
    pointers, `UseVar` copy/move (`lower/vars.rs`) runs a constructor only
    for an aggregate, and a `deinit`/`mut self` receiver's write-back
    (`lower/methods.rs`) rejects a scalar receiver.
  - Lever: pass a lifecycle operand per the callee's `FnSignature` (load a
    scalar parameter, keep an aggregate's pointer), then widen
    `register_field` to `[field] if declaration.register_passable` and
    update its unit test, `build_structs`' doc, and
    `docs/native-abi.md`'s unwrap rule. `VariadicList`, `VariadicPack`,
    and `FastMathFlag` become held too.
  - A conditional row never counts: `Tuple[Int, Int]`, `Optional[Int]`,
    and `B[T](RegisterPassable where conforms_to(T, RegisterPassable))`
    stay aggregates at the pin.
  - Probe: `W(Movable, RegisterPassable){r: R}` over that `R`, with
    `print(take(R(1)))` over `take(var r: R)` and `var w = W(R(2));
    print(w.r.v)`: the pin prints `del R 1`, `1`, `2`, `del R 2`; with
    the widening alone, Pliron fails "unsupported mutating method on a
    scalar receiver".
  - Fixture to add with it: `assets/ok/register_passable_one_field_lifecycle.mojo`,
    covering drop through a `var` parameter, a held field of a held
    struct, a tuple element, a `List` element, an `Optional` payload, and
    `.copy()` of a held struct with a user copy constructor and of a
    memory-only struct holding one (the plan file's `fx.mojo` without its
    move constructor).
  - Found while landing R1524 (2026-11-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R2040 An absent `fastmathFlags` on `pop.add`, `pop.sub`, or
  `pop.mul` writes no flags natively, where the pin's default is `contract`**

  MIR's `fastmath: None` means no flags, and Pliron's `fastmath_attr`
  (`lower/support.rs`) writes none; at the pin an absent attribute is the
  op's default property value, `contract` for these three ops.
  - Every float `+`, `-`, and `*` reaches one of them through `SIMD`'s
    operator bodies, so the change touches every native float operation.
  - `pop.fma`, `pop.cast`, and `fdiv` default to no flags at the pin, as
    Mojito already writes.
  - Lever: give `fastmath_attr` the op's default, `contract` for a float
    `binary` add, sub, or mul whose flag is absent, keeping an explicit
    `#pop.fmf<none>` flagless.
  - Risk: at `--native-opt release` on an FMA-capable CPU, LLVM fuses a
    contracted `fmul` and `fadd` into one `fma`, so a `Float64` `a * b + c`
    may print differently from the VM's unfused arithmetic, as the pin's
    own compiled code does. The heavy lane's release parity sweep finds
    such rows; they follow the pin.
  - `Float32` stays unfused until R2039, since its f64 widening puts a
    `fptrunc` between the two operations.
  - Probe: `a * b + c` over `Float32` gives `fmul contract` and
    `fadd contract` at the pin, and plain `fmul`/`fadd` (at f64) natively.
  - Probe: ``__mlir_op.`pop.sub`(a, b)`` over `Float32` gives
    `fsub contract` at the pin.
  - Found while landing R1600 (2026-11-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1768 An anonymous callable contract's own binders are named
  references, where upstream's are signature slots**

  `GenericFuncTy.decls` holds a contract's binders as `DeclRef`s under a
  per-contract `$callable:…` owner; upstream stores a function type's
  parameters as signature index refs and renames them into a declaration's
  named refs where one is opened (`replaceIndexRefsWithNamedRefs`).
  - `canonical_generic_signature` (`checker/traits_support.rs`) rebuilds the
    slot form for every comparison instead of reading it.
  - `instantiate_generic_callable_value` (`checker/generics.rs`) binds a
    contract's arguments by spelling (`value_argument_environment`, then
    `ParamBindings::from_named_values`), not by slot.
  - The consumers that move with the representation: that function, call
    inference over `GenericFunc` (`call_inference.rs`), MIR lowering of
    callable-binder calls, and `native::mono` substitution; split the entry
    on planning if they outgrow one session.
  - Found while landing R1763 (2026-10-31).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1993 A compile-time `SIMD` arithmetic dunder over a symbolic
  vector keeps its application, where upstream folds its `pop` body**

  `T[v.__add__(w)]` over `v, w: Int32` is "expected T[(v + w)], found
  T[v + w]", where the pin runs it.
  - `builtin_fold::mlir_op_operator` reads `pop.add(a, b)` over two
    storages as the infix `a + b`, but `ParamContext`'s ring
    (`arithmetic_domain`, `build_ring`) takes only the integer domains, so
    a non-`Int` `!kgen.simd` storage sum builds nothing.
  - So `SIMD.__add__`, `__sub__`, `__mul__`, `__neg__`, and the other
    `Self(mlir_value=__mlir_op.`pop.…`(…))` bodies fold no template.
  - Lever: an elementwise storage node per `pop` arithmetic op, as
    `ParamOp::SimdCmpLt` is for `pop.cmp`, whose `struct.create` of a
    field read is the source operator over the vectors.
  - Probe: `__add__` and `__neg__` over `Int32` and over
    `SIMD[DType.float32, 4]`, as type identities against the infix forms.
  - Found while landing R1574 (2026-11-07).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1994 A compile-time operator over `SIMD` operands builds its
  `ParamOp` directly, where upstream folds the operator's dunder call**

  Mojo's parser spells `a < b` as `a.__lt__(b)` and folds that call;
  `Checker::infix_value` (`checker/constraints.rs`) builds `ParamOp::Lt`
  over `SIMD` operands itself, since `SIMD` is not `struct_valued`.
  - Since R1574 the comparison dunders fold to the same nodes, so the
    comparison arm can route through `struct_operator_value` /
    `called_application` with no change in identity.
  - The arithmetic operators follow once their dunders fold (R1993).
  - Done when `infix_value`'s `SIMD` operands go through the dunder call.
  - Probe: `assets/ok/always_inline_builtin_comparison_fold.mojo`,
    `simd_comparison_dunders.mojo`, and `where n < 3` keep their output.
  - Found while landing R1574 (2026-11-07).
  - Depends on R1993.
  - Model: Opus, Not Planned.

- [ ] **R1790 A `_` in a binding's annotation is not solved by its
  initializer**

  `var v: V[DType.int32, _] = V[DType.int32, 4](7)` over a user struct
  stops with "type mismatch for value parameter 'length': expected Int,
  found a non-materializable compile-time value", where the pin prints
  `7 4`.
  - `SIMD[dt, _]` is accepted only through the positional reader
    (`Checker::positional_simd_dims`'s `SimdWidth::inferred`) and `coerces`'
    `to_width.is_inferred()` arm, which `simd_declared_application` keeps
    for a bracket holding `_`.
  - Lever: resolve an annotation's `_` slot as an inference variable the
    initializer's type solves, as a parameter pattern's is.
  - Then delete `SimdWidth::inferred`, its `coerces` arm, and the
    positional reader's hole case.
  - Probe: `assets/ok/simd_inferred_width_binding.mojo` and
    `simd_inferred_width_from_argument.mojo` over `V` instead of `SIMD`.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1833 `struct_field_types` answers no fields for a struct
  applied to its own binders, where upstream's `Self` inside a struct's
  methods is the parametric struct with its declared fields**

  An identity application (`SIMD[dtype, length]` inside `SIMD`'s own
  methods) names its own binders, which `types::struct_field_types`
  treats as unclosed.
  - So `MirDeclarations::field_types` keeps a `SIMD` arm answering a
    symbolic `SIMD`'s `_mlir_value` at its own slots.
  - Probe: deleting that arm makes `assets/ok/simd_to_bits.mojo`,
    `simd_hash_member.mojo`, and `template_method_simd_leaf.mojo` fail
    elaboration ("function instantiation of
    `_unsigned_integral_type_of` failed").
  - Lever: `struct_field_types` returns the declared fields when every
    argument is the declaration's own binder in order; then delete the
    arm.
  - Risk: it changes the answer for every generic struct inside its own
    methods (drop elaboration, SROA, `drop_runs_no_code_within`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1955 The parameter domain reads a `Bool`'s `_mlir_value` as the
  `Bool`, where upstream's is its `!kgen.scalar<bool>` storage**

  A vector's or other scalar's `_mlir_value` read is a `struct.extract`
  at its storage, but a `Bool`'s is still the `Bool` itself.
  - `Checker::scalar_storage_expression` (`checker/constraints.rs`) keeps
    a short-circuit for a `Bool` object's `_mlir_value`, and its
    `Bool(mlir_value=s)` arm reads a `Bool`-typed `s` as the `Bool`.
  - The connectives (`BoolAnd`, `BoolOr`, `BoolXor`, `Cond`) have no
    storage counterpart, so `struct.extract` and `struct.create` between
    a `Bool` and its storage must map them node by node, as
    `ParamContext::build_extract`/`build_create` map the integers'
    polynomial.
  - Probe: `def f[b: Bool](x: Tag[Bool(mlir_value=b._mlir_value)])`
    fails to check ("not a compile-time Int constant: not an associated
    comptime expression"), where `Tag[b]` runs.
  - Probe: with the short-circuit gone, std still packages and
    `assets/ok/bool_storage_primitives.mojo` and
    `assets/ok/mlir_pop_select.mojo` run.
  - Probe: `def g[b: Bool]()` building
    `Tag[Bool(mlir_value=b._mlir_value)]()` and passing it to
    `use[b](x: Tag[b])` fails to elaborate ("keeps the parameter constant
    `b._mlir_value` in elaborated MIR"), where the pin runs it.
  - Found while landing R1835 (2026-11-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1957 The parameter domain does not fold `Bool(value=i)` or
  `b.__mlir_i1__()`, where upstream's are the `i1` builtin casts beside
  `Bool`'s storage**

  Upstream's `Bool.__init__(value: i1)` is `pop.cast_from_builtin` into
  `!kgen.scalar<bool>` then `struct.create`, and `__mlir_i1__` is
  `struct.extract` then `pop.cast_to_builtin`; Mojito's parameter casts
  know only the `index` pair.
  - `ParamOp::CastToBuiltin`/`CastFromBuiltin`
    (`ParamContext::build_cast`, `param_expr/fold.rs`) cast only between
    `index` and `!kgen.scalar<index>`. Add the `i1` /
    `!kgen.scalar<bool>` pair.
  - `Checker::scalar_storage_expression`'s `Bool(value=b)` arm
    (`checker/constraints.rs`) reads a closed `i1` as the `Bool`. Replace
    it with the cast plus R1955's `struct.create`, as the
    `Int(mlir_value=…)` arm uses `int_of_index`.
  - Build `b.__mlir_i1__()` as R1955's `struct.extract` plus
    `cast_to_builtin`, as the `__mlir_index__` arm does.
  - Probe: `def f[b: Bool]()` building
    `Tag[Bool(value=b.__mlir_i1__())]()` and passing it to
    `use[b](x: Tag[b])` fails ("expected Tag[b], found
    Tag[$comptime$f$c0(b)]"), where the pin runs it.
  - Probe: `comptime x = Bool(value=True.__mlir_i1__())` still prints
    `True`, and `assets/ok/bool_storage_primitives.mojo` runs.
  - Found while landing R1840 (2026-11-06).
  - Depends on R1955.
  - Model: Opus, Not Planned.

- [ ] **R2047 An `__mlir_attr` spelled at `: i1` reads as a `Bool`, not
  an `i1`**

  Upstream types `__mlir_attr[`#pop<float_literal_cmp<eq `, a, `,`, b,
  `>> : i1`]` by its trailing `i1`; Mojito's `read_bracketed_attribute`
  (`checker/indexing.rs`) types it by the parameter value
  `bracket_operand` builds, the `Bool` comparison.
  - Probe: `take(__mlir_attr[`#pop<float_literal_cmp<eq `, a.value, `,`,
    a.value, `>> : i1`])` over `take(v: __mlir_type.i1)` and `comptime
    a = 1.5` fails ("expected __mlir_type.i1, found Bool"), where the
    pin prints `True`.
  - So `Bool(...)` over such an attribute resolves to
    `Bool.__init__[T: Boolable]`, not `__init__(value:
    __mlir_type.i1)`, in the six comparison dunders of
    `stdlib/std/builtin/float_literal.mojo`.
  - Typing the read `i1` alone fails the MIR verifier ("argument 0 of
    'Bool.__init__$ov$$$mlir$type$i1' has type Bool"): the value still
    lowers as a `Bool` register. Build it as R1957's `i1`
    `cast_to_builtin` of the comparison.
  - Found while landing R1991 (2026-11-08).
  - Depends on R1957.
  - Model: Opus, Not Planned.

- [ ] **R1958 The MIR verifier does not type `literal.materialize`'s
  result at its target, where upstream's converted attribute is the
  value's type**

  `verify_value_instruction`'s `MaterializeLiteral` arm judges the target
  and the source but never the `dest` register's type, so a storage
  conversion into an `Int`-typed register verifies.
  - Lowering and the reader's pre-1.170 upgrade both type the result at
    the `!kgen.simd` target, so the rule should reject no emitted MIR.
  - Probe: a hand-written `literal.materialize { dest: %r1, value: %r0,
    target: kgen_simd { dtype: int, width: 1 } }` with `%r1` typed `Int`
    passes `mojito opt`.
  - Found while landing R1837 (2026-11-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1618 A compile-time argument computed by an inlined dunder keeps
  its function's parameter**

  `subscript_expr_fwd[n]`, returning `Grid(7)[n + 1]` over a
  `__getitem_param__[i]` that never reads `i`, keeps `n` and makes one
  instance per `n`, and so does `subscript_div_fwd[n]` over
  `Grid(5)[10 // n]`.
  - Since R1551 `n + 1` is `SIMD.__add__`'s inlined body, a region whose
    result is the argument, and since R1566 `10 // n` is
    `SIMD.__floordiv__`'s, with its asserts. The forward analysis
    (`mir::param_uses`) traces neither into the removed binder.
  - Upstream folds the `@always_inline("builtin")` dunder into the
    parameter expression `n + 1`, which forwards as any other, and never
    emits a parameter argument's operation, so `10 // n` at `n == 0` fails
    nothing.
  - `forwarded_parameters_leave_the_instance_key` in
    `tests/compiler_test.rs` expects two instances of each until this
    lands.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1619 A local `comptime` bound to an `IsTrivially*` predicate is
  rejected**

  `comptime trivially = IsTriviallyMovable[Int]` inside `main` fails with
  "a generic comptime alias must be declared at module scope", where the
  pin prints `True`.
  - The application parses as a type application, so the local statement
    takes the alias route (`check_generic_comptime_alias`), which refuses
    any alias inside a function.
  - Routed as a value instead, `IsTriviallyMovable[…]` types as a type
    (`infer`'s `TypeApply` arm), and the compile-time path reads the name
    as an unknown type. Only `compile_generic_constraint` knows the
    predicate.
  - Pinned by `conformance/probes/trivially_predicate_local_comptime.mojo`.
  - Probe: a local `comptime f = seven` naming a function, then `comptime
    t = f()`, takes the same alias route and fails the same way, where the
    pin prints `7`. So does `comptime f = inner` over a nested `def`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1620 A moved-in pack capture forwarded whole fails MIR verification
  instead of the ownership check**

  `def relay() {args^} -> Int: return score(40, *args^)` inside a pack
  generic is rejected by the MIR verifier after `canonicalize` ("drop.var
  of slot $v1 ('args') that some path reaching it does not hold"), where
  the ownership check should report "'args' is uninitialized at return",
  as the pin rejects it.
  - The program is rejected either way: the diagnostic and the phase are
    wrong.
  - Pinned by `captured_outer_pack_forwarding_must_refill_the_captured_pack`
    in `tests/comptime_test.rs`, failing until this lands.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1621 A capture-list closure over a pack element in a `comptime for`
  fails specialized MIR verification**

  `comptime for i in range(Ts.length): var x = args[i]; def show() {x}:
  print(x)` over `*args: *Ts` fails with "variable slot 4 retains ABI-only
  RuntimePack type outer$show$__storage$mono…", where the pin prints each
  element.
  - The failing slot is in `outer$mono$T$pack$Int$Bool`, the instance
    over the whole pack, and its type is the closure's storage struct.
  - Pinned by `cloned_comptime_bodies_keep_distinct_checked_occurrence_facts`
    in `tests/comptime_test.rs`, failing until this lands.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1407 An `enum`'s `__Enum` members are generated on each enum,
  where upstream's are `__Enum`'s trait defaults**

  `mojito_parser::enums` writes `__enum_init`, `__get_enum_storage`,
  `_get_enum_discriminant`, and `_unsafe_get_enum_payload` into every enum,
  and each case constructor writes its own storage, where upstream's
  `std/builtin/_enum.mojo` declares the four as defaults of `trait
  __Enum(EnumLike)` and the case constructors call `__enum_init[id,
  Case](payload0^, ...)`.
  - Upstream's `__enum_init` is a `@staticmethod` with an `out` result over
    a `var *payload: *Ts` pack, writing each element in place through
    `consume_elements` and a capture-list closure, and two of the others
    return `ref[self]`.
  - Move the four into `_enum.mojo` as defaults, delete their generation and
    `comptime::synth::stamp_enum_storage_writes`, and have each case
    constructor call `__enum_init` as upstream's do.
  - Found while landing `enum` (2026-10-25); see `docs/notes/enums.md`.
  - Depends on R942 (static trait members), R549 (`out` results in
    defaults), R571 (`ref`-returning defaults), R1124 (a runtime
    `consume_elements` handler), and R1378 (a capture-list closure as a
    value).
  - Model: Fable, Planned.

- [ ] **R1408 `__EnumStorage` is a compiler intrinsic with a `u32` tag,
  where upstream's is a library struct over `!kgen.variant`**

  An enum's storage is the intrinsic `TyKind::Variant` its operations select
  by case index (`__EnumStorage[...](payload, index=i)`, `storage[i]`,
  `get_discriminant()`), where upstream's `__EnumStorage[case_types]` is a
  struct in `std/builtin/_enum.mojo` over `!kgen.variant`, read through
  `pop.variant.discr_gep` and `pop.variant.bitcast`.
  - Its tag is the variant's `u32`, where upstream's
    `__enum_discriminant_dtype` picks `u8`, `u16`, or `u32` by case count,
    so `size_of` of an enum differs from the pin's.
  - Port upstream's struct, with its conditional conformances and lifecycle
    bodies, once `__mlir_type` and `__mlir_op` accept the variant type and
    ops, and delete the intrinsic's checker arms.
  - Probe (from R1868): the bundled `Variant.set(init_with=)` stores the
    factory's result through `self._storage.set[T](init_with())` and
    `deinit_with` hands its handler `self._storage.unsafe_unwrap[T]()`,
    where upstream's `_DefaultVariantStorage` writes in place through
    `unsafe_ptr[T]().unsafe_write(init_with=)` and destroys through
    `unsafe_ptr[T]().unsafe_deinit_pointee_with(deinit_func)`.
  - Found while landing `enum` (2026-10-25).
  - Depends on R1392 (`pop` ops in `__mlir_op`), R221
    (`Pointer.unsafe_bitcast`), and R1316 (`!kgen.pointer` values).
  - Model: Fable, Planned.

- [ ] **R1409 An enum's default `Writable` text is generated per enum, where
  upstream's `_enum_reflection_write_to` writes any `EnumLike`**

  `mojito_parser::enums` generates `write_to` and `write_repr_to` for an
  `enum` declaring `Writable` without them, where upstream's `Writable`
  defaults call `_enum_reflection_write_to` for every `EnumLike` conformer,
  `Optional` included.
  - The generated bodies have upstream's shape (`Name.case(payload)`), so
    the text matches.
  - `Writable`'s defaults are now the trait's own bodies, so deleting the
    generation leaves an enum to `_enum_reflection_write_to`, which a
    hand-written `EnumLike` conformer already runs
    (`assets/ok/enum_like_callable_binder_cases.mojo`).
  - Found while landing `enum` (2026-10-25).
  - Depends on R1879 and R1880.
  - Model: Opus, Not Planned.

- [ ] **R324 `reflect[T]` is a compiler builtin where upstream's is a
  stdlib struct over parameter attributes**

  Problem: Mojito answers `reflect[T]` queries by built-in rules in the
  checker (`checker/reflection.rs`) and the elaborator
  (`mono/symbolic.rs`), where upstream's `Reflected[T]`
  (`std/reflection/reflect.mojo`) is a stdlib struct whose methods return
  parameter attributes its elaborator folds.
  - `field_count()` is `_field_types_of[T]().length` over
    `#kgen.struct_field_types`, `is_struct()` is `#kgen.is_struct_type`, and
    `field_index[name]()` is `#kgen.struct_field_index_by_name`.
  - The shared policy is one function today (`ReflectQuery::answer`,
    `param_expr.rs`), so the answers agree; the shape does not.
  - Closing it means a bundled `Reflected` over intrinsics that lower to
    `ParamKind::Reflect`, with the built-in rules deleted.
  - `std/reflection/__init__.mojo` exports the builtin from a
    docstring-only `reflect.mojo`; the rest of upstream's root
    (`Reflected`, `reflect_fn`, the locations) is not bundled.
  - `name()` and `base_name()` already answer upstream's
    `#kgen.get_type_name` and `#kgen.get_base_type_name` and construct the
    `StaticString` its methods do (`static_string_of`,
    `checker/reflection.rs`).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R2028 `Tuple`'s storage is a non-copyable
  `storage: __RuntimeTuple[*Self.Ts]`, where upstream's is
  `_mlir_value: !kgen.struct`, a bit-copied MLIR value**

  Upstream's trivial copy fast path is `self._mlir_value = copy._mlir_value`;
  Mojito's must call `self.storage = copy.storage.copy()`, because the
  storage at a symbolic pack is not `ImplicitlyCopyable`.
  - Probe: with `self.storage = copy.storage`, the bundled library does not
    package ("value of type 'Tuple[*Ts]' cannot be implicitly copied").
  - The field is named `storage`, so `reflect[Tuple[Int]]` names a field
    the pin calls `_mlir_value`.
  - Renaming it collides with the compiler's `ct::MLIR_VALUE_FIELD`
    special cases, and the VM spells `"storage"` for tuples
    (`vm/adapters.rs`, `vm.rs`, `crossing.rs`).
  - Upstream spells the type
    `__mlir_type[\`!kgen.struct<:\`, type_of(Self.Ts.values), Self.Ts.values, \` isParamPack>\`]`.
  - Found while landing R359 (2026-11-08).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R360 `TString` owns a `Tuple` of its parts, where the pin's borrows
  a pack beside its encoded literal text**

  Problem: Mojito's `TString[*Ts: Movable & Writable]` stores every literal
  segment and every interpolation in an owned `Tuple[*Self.Ts]`, where the
  pin's `TString[origins: ImmOrigin, //, *Ts: Writable]` borrows a
  `VariadicPack` of the interpolations and keeps the literal parts
  NUL-encoded beside it (`__make_tstring`).
  - An interpolated place whose type is not `ImplicitlyCopyable` is
    formatted to a `String` when the t-string is created, where the pin
    borrows it.
  - Mojito's `__make_tstring[*Ts](var *args: *Ts)` takes the literal
    segments as interleaved `String` arguments, where the pin's takes one
    `format_string` parameter beside the interpolations.
  - The pin's shape needs an origin-carrying struct over a borrowed pack.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R2025 `Tuple`'s pack initializer stores `__RuntimeTuple(*args^)`
  where upstream consumes its owned pack with `consume_elements`**

  Upstream marks `self._mlir_value` initialized, declares
  `def init_elt[idx: Int](var elt: Self.Ts[idx]) {mut self}` running
  `Pointer(to=self[idx]).unsafe_write(elt^)`, and calls
  `args^.consume_elements(init_elt)`.
  - With that body the bundled library does not package: "no symbolic
    rule for an unbound pack or reflected type: method 'consume_elements'
    of an unbound pack".
  - `var *args: *Ts` is still the tuple of its values (R1927), and the
    bundled `VariadicPack` declares no `consume_elements` (R1924).
  - Do not add a symbolic `consume_elements` rule for the tuple collector.
  - Split from R341 (2026-11-08), whose copy, move, default, and
    `init_with` initializers landed.
  - Depends on R1927.
  - Model: Fable, Planned.

- [ ] **R2024 `Tuple(init_with=)` writes each element with `unsafe_write`
  where upstream constructs it in place through
  `__get_address_as_uninit_lvalue`**

  Upstream assigns `__get_address_as_uninit_lvalue(Pointer(to=self[i])._mlir_value) = init_with[i]()`,
  so an element need not be `Movable`; Mojito's
  `Pointer(to=self[i]).unsafe_write(init_with[i]())` needs `Movable`.
  - `__get_address_as_uninit_lvalue(p) = v` is not a builtin.
  - Lever: type it beside R1924's two builtins (`checker/operators.rs`).
  - Blocks R1121.
  - Split from R341 (2026-11-08).
  - Depends on R1924.
  - Model: Opus, Not Planned.

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

- [ ] **R1676 The `pop.array` ops reject a register-passable element whose
  copy or destruction runs code, where the pin copies its bits and never
  destroys an array's elements**

  `pop.array.repeat` of a `RegisterPassable` struct with a `copy=`
  initializer and a `__del__` is "outside the subset", where the pin runs it.
  - The pin's ops copy an element's bits, running no copy initializer, and
    a `!pop.array` never destroys its elements.
  - Probe: `repeat` of `R(7)` into `!pop.array<2, R>`, then `get` at index 1
    and `print(b.x)`. The pin prints `del 7` (the operand temporary), `7`,
    and `del 7` (the result).
  - Mojito admits only trivially register-passable elements
    (`pop_array_element`, `checker/method_calls/pop_array_ops.rs`).
    `mir::drop_runs_no_code_within` derives an array's drop from its
    element, so admitting one would destroy the elements.
  - Lever: a `!pop.array` drop that runs no code whatever its element, and
    ops that bit-copy on both backends. `StaticTuple`'s
    `_StaticTupleTraits` admits such elements.
  - Probe: over an element binder bounded `RegisterPassable`
    (`struct A[T: ImplicitlyCopyable & Deinitable & RegisterPassable, n:
    Int]` repeating `fill`), `A[R, 2](R(3))` fails the elaborator's
    per-instance rule (`Specializer::verify_array_elements`), where the pin
    runs it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1694 `CompilationTarget.stdlib_plugin` names `DefaultPlugin`
  directly, where upstream's selects through `_first_conforming_plugin`**

  The bundled member is `DefaultPlugin.HookFor[Self()]`, where the pin's is
  `Self._first_conforming_plugin[StdPlugin,
  default=DefaultPlugin].HookFor[Self()]` over `TargetPluginSelector`.
  - The two agree on every target without an `opaque_plugin` payload,
    which is every target Mojito builds.
  - Port `TargetPluginSelector` and `PluginSelector` verbatim into
    `stdlib/std/_plugin/selector.mojo`, and `_first_conforming_plugin`,
    the verbatim member, and the `TargetPluginSelector` import into
    `stdlib/std/sys/info.mojo`. Both files' headers list them.
  - The checker reads `#kgen.get_target_plugin<target, : list-type
    default-list> : list-type` (`mlir_bracket_attribute`). The elaborator
    answers it from the target's `opaque_plugin` payload, else the default
    list, so on every `NativeTarget` it is the default list.
  - `KgenTarget` (`mojito-types/src/ct.rs`) gains an optional
    `opaque_plugin`, a MIR text and package format bump.
  - Probe: `assets/ok/std_plugin_hooks.mojo` keeps printing the pin's
    output.
  - Depends on R1695, R1696, R1700, R1697, R1701, and R223 (`downcast`).
  - Model: Fable, Planned.

- [ ] **R1698 `PluginHooks`'s static hooks `abort_fn` and
  `debug_assert_emit_fn` are undeclared**

  The bundled `stdlib/std/_plugin/_trait.mojo` omits both `@staticmethod`
  requirements, so `os.abort` cannot call `CurrentPlugin.abort_fn()` as
  upstream's `os.mojo` does.
  - Port both verbatim, with their default bodies.
  - Call `CurrentPlugin.abort_fn()` from the bundled `abort`, and read
    `_handles_debug_assert` in `_debug_assert_msg` if the bundled
    `debug_assert` has that function.
  - Probe: `trait H:` declaring `@staticmethod def abort_fn(): pass` is a
    parse error ("A method's first parameter must be 'self'").
  - Depends on R942.
  - Model: Opus, Not Planned.

- [ ] **R1702 `PluginHooks.reduce_generator_fn` is undeclared**

  The bundled `stdlib/std/_plugin/_trait.mojo` omits the hook and its type,
  `_ReduceGeneratorPluginHookFnType`, where upstream declares both.
  - Port both verbatim, with the imports only they use.
  - The type's signature needs `IndexList` (R1674) and
    `std.utils.coord.Coord` (R1703).
  - It composes `ImplicitlyCopyable & RegisterPassable & def[…](Coord) ->
    …` as a bound inside a function type's own binder list, and has an
    `IndexList[_, element_type=DType.int64]` hole there.
  - Probe: `comptime if CurrentPlugin.reduce_generator_fn:` prints the
    else arm at the pin and reports "has no field 'reduce_generator_fn'"
    in Mojito.
  - Upstream reads the hook only in MAX (`algorithm/reduction.mojo`), so
    no bundled `std` read site waits on it.
  - Depends on R1674, R1703.
  - Model: Opus, Not Planned.

- [ ] **R1481 `Equatable`'s and `Hashable`'s reflective defaults are
  synthesized per conformer, where upstream's are the traits' own bodies**

  `synthesize_equatable_eq` and `synthesize_hashable_hash`
  (`comptime/synth.rs`) give each conformer without its own `__eq__` or
  hasher-fed `__hash__` a body comparing or hashing each field, where
  upstream's `Equatable.__eq__` and `Hashable.__hash__` are default bodies
  over `reflect[Self]` (`std/builtin/comparable.mojo`,
  `std/hash/hash.mojo`).
  - `reflect[Self]` in a plain struct's method already reads the fields.
  - The bodies read each field through `r.field_ref[i](self)`, which now
    checks in a trait default, and assert its conformance with the message
    the bundled `_field_conforms_to_error` builds, as `Writable`'s do.
  - The bundled traits' requirements stay `...` until both check.
  - Done when both traits declare upstream's bodies and the two synthesis
    passes are gone.
  - Found while landing R1383 (2026-10-26), refiled from R269.
  - Depends on R77 and R1210.
  - Model: Fable, Not Planned.

- [ ] **R1616 A conformer's mismatched method keeps the trait's default out**

  An `Equatable` struct declaring `__eq__(self, var other: Coin)` and
  `__ne__(self, var other: Coin)` is rejected, "method '__ne__' does not
  match the signature required by trait 'Equatable'", where the pin runs it.
  - At the pin neither method satisfies the `other: Self, /` requirement,
    so the trait's defaults fill both, and the struct's methods are
    overloads beside them.
  - `trait_defaults::inherited_defaults` skips a default whose overload
    key a struct method spells, whatever its conventions, and the
    conformance check then finds no matching witness.
  - Mojito synthesizes `__eq__` per conformer that declares none (R1481),
    so `__eq__` needs the trait's own default body first.
  - Pinned by `conformance/probes/equatable_by_value_operand_overloads.mojo`.
  - Depends on R1481 (the reflective `__eq__` default).
  - Model: Opus, Not Planned.

- [ ] **R1883 `print(flush=True)` does not call upstream's `_flush`**

  The bundled `print` (`stdlib/std/io/io.mojo`) takes `flush` and ignores
  it, where upstream's ends with `if flush: _flush(file=file)`, which
  `fflush`es a `fdopen`ed `dup` of the descriptor.
  - Needs `std.sys._libc`'s `dup`, `fdopen`, `fflush`, and `fclose` in
    `mojito_types::ffi::CALLEES`, the VM's libc shim (`vm/libc.rs`), and
    `io.mojo`'s `_fdopen` context manager.
  - Output is unchanged: nothing in Mojito writes through C stdio.
  - Probe: with `flush` unread, the elaborator removes the argument, which
    exposed the unroller forgetting a removed argument's shared default
    (`mono/unroll.rs`, fixed with R1287): `print` in a `comptime for` body
    after a `print` before it.
  - Found while landing R1287 (2026-11-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1884 The bundled `print` omits upstream's plugin and GPU branches**

  Upstream's `print` writes through `_FixedWriteBuffer` to
  `CurrentPlugin.print_emit_fn` when a plugin supplies one, and rejects a
  GPU target, where the bundled one has only the CPU branch.
  - Needs `_FixedWriteBuffer`, `CStringSpan`, `_PrintEmitPluginHookFnType`,
    and the hook in `stdlib/std/_plugin/_trait.mojo`, whose header omits
    `print_emit_fn` on purpose.
  - Spell the selection as `std.math` does (`comptime hook = …; comptime
    if hook:`) until `comptime if case` lands.
  - Probe: upstream's `values._write_to(buffer, sep=sep, end=end)` is
    respelled as a `comptime for` over the pack, since the bundled
    `_write_to` takes `StaticString` delimiters where `print`'s are
    `StringSlice`s (R1926).
  - Found while landing R1287 (2026-11-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R426 The bundled `_FormatUtils` respells constructs upstream writes
  differently**

  Problem: the port of upstream's `collections/string/format.mojo` in
  `stdlib/std/string.mojo` matches the pin's behaviour but not its text,
  because Mojito rejects several of upstream's spellings.
  - `_FieldVariantType = Variant[StringSlice, Int, NoneType, Bool]` is an
    `Int` kind tag plus an `Int` value, the kinds module-level `comptime`
    constants (R428, R436).
  - `ord("{")` and friends are `UInt8(123)` runtime locals (R427), and the
    `supported_conversion_flags` SIMD membership test is two comparisons
    (R135).
  - `Error("Index ", val, " not in *args")` concatenates (R429), and the
    `comptime l_err`/`r_err` texts are inlined (R435).
  - The nested `_format` closure is a method of `_FormatCurlyEntry` (R288),
    and its `ref arg = args[i]` reads `args[i]` at each use (R438).
  - `Int(field)` is `atol(String(field))` (R430), `fmt_bytes.unsafe_get(i)`
    is `fmt_bytes[i]` (R431), and `_build_slice` is a `[byte = a : b]`
    slice.
  - `x and bytes[i] == …` and `manual and automatic` (Int truthiness) are
    nested `if`s and `!= 0` tests (R97); `for i in range(n)` is a `while`
    loop (R243); a `raised_kwarg_field = String(f)` store spells
    `Optional[String](…)` (R437).
  - `_PrecompiledEntriesRuntime` holds no `format` view and no origin
    parameter: a struct field over a `def`'s auto-parameterized
    `StringSpan` origin does not bind, so `format_precompiled` takes the
    template beside the entries. The span-backed `_PrecompiledEntries`
    and `compile_entries_runtime_no_raises` are omitted, and
    `format_to_comptime` parses at run time, until R425.
  - Found while landing R421 (2026-10-06).
  - Depends on R427, R428, R429, R430, R431, R435, R436, R437, R438, R288,
    R135, R97, R243.
  - Model: Opus, Not Planned.

- [ ] **R445 `ConstructTypeParam` is a special case of `ConstructType`**

  `T()` and `Ts[i]()` are carried by `MirInstr::ConstructTypeParam` (a
  binder, plus a pack index), while `types[i]()` is carried by
  `MirInstr::ConstructType` over the type expression, where the pin has
  one operation: the `Defaultable` initializer of a type-valued parameter
  expression.
  - Folding the nullary forms gives `T()` the operand `Ty::Param` and
    `Ts[i]()` the dependent selection, closed by the elaborator's one
    `ConstructType` arm (`mono::substitute::default_construct_parameters`).
  - Nothing waits on the VM: it runs concrete MIR only, and rejects both
    instructions as parametric (`backend/vm/exec.rs`).
  - Lowering sites: `expr_call.rs`, `expr_method.rs`, and
    `type_construction.rs` in `mojito-mir`; `inline_parametric`'s
    `constructs_binder` veto goes with the form.
  - `T(copy=x)` keeps its own form, or becomes a selected `Copyable`
    initializer call.
  - Probe: `inline-parametric` keeps the `each[Int, Bool]()` call over
    `comptime for i in range(len(Ts)): print(Ts[i]())`, and `one[String]()`
    over `print(T())` (bounds `Defaultable & Writable & Deinitable`), since
    the inlined construction would name the callee's binder, which the
    caller does not declare, though substitution already closes its type.
  - Found while landing R364 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1431 A nested `def` without a capture list stays a generator
  nested in its enclosing declaration, where the pin promotes it to a
  module-scope function**

  A capture-free nested `def` (and a thin lambda) lowers to `outer$inner`,
  a generator whose declaration names `outer` (`enclosing`) and which the
  elaborator instantiates under each instance of `outer`
  (`Specializer::instantiate_nested_bodies`), where upstream's
  `constructClosure` calls `promoteClosure(decl, paramCaptures)`: a
  module-scope function whose parameters are the enclosing binders it
  names, then its own.
  - The binder collection is the check's `closure_uses`
    (`crates/mojito-checked`), which a capturing closure's storage
    already uses; it records uses only for a closure with storage.
  - Probe: `def outer[n: Int]() -> Int:` declaring `def helper() -> Int:
    return n * 2` lowers `outer$helper` with `enclosing: outer`; the pin's
    `helper` is a top-level function parameterized on `n`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1944 A closure storage's field types name the enclosing places
  their captured values carry, where upstream promotes every captured
  origin to an origin parameter of the storage**

  `Checker::closure_storage` promotes only a reference capture's own place
  (`StorageReference`), so a captured value whose type carries an origin
  (`Span[Int, origin_of(xs)]`, `Pointer[Int, origin_of(k)]`), and a
  sibling capture's referent `S_f[origin_of(k)]`, keep the enclosing
  place in the storage's field types, which its `__init__` and method are
  verified against.
  - Promote each origin a field's type carries to its own origin
    parameter, and apply it at the closure's value
    (`CheckedClosureStorage::origin_arguments`).
  - Probe: `var p = Pointer(to=k)`, `def f() {p} -> Int: return p[]`,
    `print(f())` fails MIR verification ("MakeRef destination targets
    Pointer[Int, origin@…], incompatible with place storage Int") where
    the pin prints `1`.
  - Probe: `var s = Span(xs)` over `var xs = [1, 2, 3]`, then `def f()
    {s} -> Int: return s[0]` (or `{imm s}`) and `print(f())` fails MIR
    verification ("MakeRef destination recovers permission unavailable
    through its source capability") where the pin prints `1`.
  - Found while landing R1894 (2026-11-05).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1943 A closure's explicit `f.__call__(x)` is rejected, where
  upstream calls its storage struct's `__call__` method**

  The checker's storage struct for a closure
  (`Checker::register_closure_storage`) declares no `__call__` method, so
  a method call naming it fails ("has no method '__call__'"), where
  upstream's storage declares the method its call node resolves.
  - Give the struct a `__call__` `MethodSig` built from the closure's own
    signature (names, defaults, conventions, type parameters).
  - The lift step retargets only a `Call` or `Invoke` through a closure
    (`CheckedTables::storage_calls`); it must also retarget a method
    call on the nested struct to the lifted struct's `__call__`.
  - Probe: `var k = 3; def f(x: Int) {k} -> Int: return x * k;
    print(f.__call__(2))` prints `6`, as at the pin.
  - Found while landing R1893 (2026-11-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1945 A nested `def` without a capture list calls a sibling
  capture-list closure, which the pin rejects**

  Problem: `def m(v: Int) -> Int: return k(v) * 2`, beside a sibling
  `def k(v: Int) {x} -> Int`, passes the check and then fails MIR
  verification ("a call of 'main$k$__storage.k' leaves out an argument";
  "closure environment ... binds neither a leading reference parameter nor
  a leading `read` one"), where the pin rejects it at `k(v)` with "Could
  not infer capture convention of the captured value k".
  - `Checker::record_permitted_call_capture` (`checker/scopes.rs`) records
    nothing when no capture policy names the sibling, and the call goes
    on, so `m`'s frame reaches `k`'s storage-bearing body through
    `load_nested_closure`, which builds a `closure.make` over `k`'s
    captures for its method.
  - A thin sibling (`def k(v: Int) -> Int` with no captures) stays legal
    there, as at the pin, which prints `4` for it.
  - Probe: the program above, and the same `m` passed as a value
    (`var mm = m`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1946 MIR still lowers a nested body without storage that
  captures, which no accepted program declares**

  Every capture-list `def` and capturing lambda is its storage struct, and
  a list-less `def` captures nothing, so the storage-less capture
  lowering that `@parameter` closures used is reached by no checked
  program the pin accepts (R1945 removes the one invalid program that
  still reaches it).
  - Delete the leading reference parameters a body without storage takes
    for its captures (`mir/nested.rs`: `capture_parameters`,
    `capture_slots`' parameter branch, the `storage.is_none()` arms of the
    lifted declaration's ownership and reference flags).
  - Delete `calls_nested_directly`'s storage-less capture arm and
    `lower_direct_nested_call`'s loaded-capture arguments
    (`mir/lower_expr/calls.rs`), and the captured places
    `emit_nested_closure` puts on a `closure.make`.
  - Make `mir::verify` reject a `closure.make` with captures, replacing
    `capture_binds` (`mir/verify/instr.rs`).
  - Probe: a `closure.make` with captures after lowering any program
    (none should remain); `assets/ok/closure_storage_values.mojo`,
    `nested_def_capture_storage.mojo`, and
    `generic_nested_def_captures.mojo` still run.
  - Depends on R1945.
  - Model: Opus, Not Planned.

- [ ] **R1947 `closure.make` carries a capture list no lowering fills**

  Once R1946 lands every `closure.make` has empty captures, as upstream's
  function value of a thin body carries no environment, but each consumer
  still models one.
  - Drop `MirInstr::MakeClosure::captures` and `MirClosureCapture`
    (`mir/ir.rs`), its MIR text field, and the passes and analyses that
    read it (`inline_parametric`, `canonicalize`, `mem2reg`, `loans`,
    `moves`, `interior`, `scan`, `register_ends`, `verify/places.rs`).
  - Drop the captured-place splice of a devirtualized call
    (`Specializer::capture_arguments`, the `captures_are_empty` flag of
    `mir::callable_targets`) in `native/mono`.
  - Drop the VM's closure environment (`vm/exec.rs`) and Pliron's
    environment slots (`pliron/lower/support.rs`,
    `pliron/lower/closures.rs`), keeping the two-word callable value's
    layout until a thin function value needs no environment word.
  - Respell the pass cases that build a capturing `closure.make`
    (`inline-parametric/capturing_callable_binder`,
    `canonicalize/dead_reference_closure`,
    `mem-2-reg/reference_closure_slot`).
  - Depends on R1946.
  - Model: Opus, Not Planned.

- [ ] **R538 Storage paths for reference-valued elements outlive the last spelling of one**

  Problem: the checker, MIR, VM, and native backend keep paths for an element whose type is itself `ref[o] T`, though `List[ref[o] T]` and `Tuple[ref[o] T]` are now rejected at parse, as at the pin.
  - Sites: `infer_list_construction` and the `Tuple` arm of `infer_storage_value` in the checker, the element-slot comments in `crates/mojito-mir/src/mir/lower_stmt.rs` and `mir/verify/instr.rs`, `MirInstr`'s element-handle note in `mir.rs`, and `lower_make_ref` in `crates/mojito-pliron/src/lower/ctors.rs`.
  - Most of these also serve a struct with a direct `ref` field as an element, so delete only a branch whose sole trigger is a top-level reference element.
  - A bare `List[ref Int]` still parses and is rejected by the checker, so a `Ty::Ref` element type can still be formed until it is rejected.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R695 `Set`'s literal constructor moves its elements in, where
  upstream's borrows them and copies each**

  Upstream's `Set.__init__(out self, *ts: Self.T, __set_literal__:
  NoneType = None) where Copyable and Deinitable` adds `t.copy()` per
  element, where std's takes `var *values` and requires only `Deinitable`.
  - A set display calls this constructor since R1338, so its shape is the
    display's.
  - Probe: `var s: Set[M] = {M(1), M(2)}` over a `Movable`-only `M` runs
    here and is rejected at the pin (no viable `Set.__init__`).
  - Probe: `{x, y}` over locals that are not `ImplicitlyCopyable` is
    rejected here and borrowed at the pin.
  - Probe: `{Token(3), Token(3)}` destroys `3` twice here and four times
    on the pin
    (`tests/vm_test.rs::discarded_set_elements_and_replaced_dictionary_values_are_destroyed`).
  - Lever: give `stdlib/std/collections/set.mojo` upstream's constructor,
    update that test's count, and add the `Movable`-only display as a
    `type_error` fixture.
  - Depends on R1414 (the display must bind its elements to a read
    collector).
  - Model: Opus, Not Planned.

- [ ] **R583 The VM resolves a callee by name at run time, scanning every
  concrete function**

  Problem: `Prog::{runtime_method_name, index_of}`
  (`mojito-vm/src/backend/vm.rs`) walk the whole concrete program for each
  dispatch they resolve, while upstream resolves every call at compile
  time and the VM runs concrete MIR the elaborator already resolved.
  - `index_of` scans `mir.functions` for the exact name on every call.
  - The lever is to have the elaborator record each concrete call's
    target, so the VM resolves no name; failing that, an index by name.
  - `method_call` resolves a method by the receiver's runtime struct
    name, so it reboxes a struct the VM holds as its one field from the
    receiver's static type (`MethodInvocation::receiver_ty`), as it
    reboxes a builtin method's operands. The bridge goes when the call
    names its target.
  - Found while landing R580 (2026-10-10), which moved the elaborator's
    resolution onto by-name lookups.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R751 Explicit-destruction obligations are checked by an AST walk
  where upstream checks them in its CFG lifetime pass**

  Problem: `explicit_destroy.rs` walks the checked AST and models raise
  edges, operand consumption, and `try` joins by hand, where upstream
  reports abandonment from its lifetime pass over the lowered CFG.
  - Every edge the CFG has must be re-modelled in the walk: R749 added raise
    points, in-flight operands, and handler joins this way.
  - MIR already carries those edges, but moving the check below the
    `CheckedProgram` handoff is a waist decision (AGENTS invariant 5).
  - Its plan must split the move into one-session slices, each filed as
    its own entry before any lands.
  - Found while landing R749 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

Upstream's generators keep control flow structured, as `hlcf` region ops,
until `LowerControlFlow` after elaboration, where Mojito's HIR builds a CFG.
The entries below close that gap, staged in
[`docs/structured-control-flow-plan.md`](structured-control-flow-plan.md):
the CFG lowering point moves down one consumer at a time, and the
follow-ups the landed producer stages filed come last.

- [ ] **R1261 The MIR verifier never checks that a register's definition
  dominates its reads, where MLIR's verifier rejects an operand that does
  not dominate its use**

  A structured body can read a register defined in a region it is not
  inside, and the verifier accepts it.
  - Upstream's region ops are SSA: a value defined in a region, a region
    argument included, is visible only in that region and those nested
    in it, and `mlir::verify` reports "operand does not dominate this use".
  - Mojito's `mir::verify` checks types and region terminators, but no
    definition-before-use rule, so the error surfaces at run time.
  - Probe: a `match.next` carrying a register defined in an earlier `if`'s
    elif condition region verifies; when the condition region does not
    run, the VM reads `None` ("operator Add is not defined for None and
    Int"), found while landing R1259 (2026-10-21).
  - Probe: a read of an `if`'s, `loop`'s, or `match`'s region argument
    after the op verifies.
  - Probe: a `comptime.for` whose operand is defined after the op
    verifies, and the VM reads `None` for it (found while landing R1260).
  - The fix is a scoped walk over each structured body, each region
    seeing the registers defined before it in the regions enclosing it
    plus its own arguments.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1262 `sccp` follows elif conditions and `match.next`s to the
  regions they enter, where upstream's `SCCP` gives an `if` with elifs and
  a `match` its generic treatment**

  Mojito's `sccp` enters only the arm a constant elif condition takes, and
  a `match` case only from an executable `match.next`, meeting each region
  argument with the values carried in.
  - `SCCP.cpp`'s `processControlFlowNode` sends a multi-arm `hlcf.if` to
    the generic path ("fall through to the generic path until that is
    supported") and has no `hlcf.match` case.
  - The generic path processes every region and puts each region argument
    and result at its entry state.
  - Mojito's form is sound but folds what the pin does not.
  - R1260 already leaves `match` results overdefined, as upstream does.
  - Probe: `tests/mir_passes/sccp/elif_argument_constant.mir` and
    `match_next_argument_constant.mir` fold where the pin keeps.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1227 A comprehension lowers in MIR as an expression, so its
  clause cannot run a sequence or place target**

  Problem: `[f for [f, *_] in rows]` and `[q[0] for q[0] in xs]` are
  rejected ("a sequence or place target in a comprehension clause"),
  where the pin prints `2 1 4` and `2 7 8 8`.
  - Upstream's `emitComprehensionsAnd` (`ParserStmts.cpp:5466`) emits
    each clause as `emitForStmt`, the rest of the clauses as its body, so
    a clause target is a `for` target.
  - Mojito lowers the comprehension whole in MIR
    (`comprehension_clauses` in `lower_expr/ctrl.rs`), so a clause runs
    only a tuple target's extraction plan (`CheckedComprehensionUnpack`),
    never the statements R1136/R1137's sequence desugar and a place store
    are.
  - Lever: lower a comprehension as a nest of `for` statements through
    HIR, so a clause target is the `for` statement's desugar.
  - Probe: `conformance/probes/comprehension_sequence_target.mojo`.
  - Found while landing R1138 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R892 HIR's block arena is a `petgraph` `StableGraph` that holds no
  edge**

  Since R873 no HIR terminator jumps between blocks, so `Cfg::g` is only a
  block store, and `Cfg::node_count`/`edge_count` describe a graph nothing
  builds.
  - Lever: a `Vec<BasicBlock>` in `Lower` and `Cfg`, with `BlockId` an
    index, and `mojito-hir`'s `petgraph` dependency dropped.
  - Callers: `mir::reassigned_names` walks `cfg.g.node_indices()`;
    `tests/hir_test.rs` and `tests/mir_test.rs` use the counts.
  - Upstream has no HIR, so this only removes a shape upstream lacks.
  - Found while landing R873 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R912 A removed parameter stays in its generator's signature and its
  callers' arguments**

  Upstream's `RemoveUnusedParams` rewrites the generator to drop the
  parameter and rewrites each caller, where Mojito records
  `MirFunction::removed_params` and the elaborator keys instances without
  it.
  - Every caller still binds the argument, and the instance binds a value
    its body never reads: `remove-unused-params` over `def f[n: Int](x:
    Int)` called as `f[3](1)` and `f[4](1)` records `removed_params: [f
    slot 0]` and keeps both callers' `instantiated_args`.
  - The pass (`analysis/unused_params.rs`) already walks the call graph
    callees first; it rewrites one function at a time, so the rewrite of
    the declaration's `param_decls` and of every `param_arg` and
    `instantiated_args` naming it needs a whole-program step after the walk.
  - The elaborator binds a removed parameter of a compile-time application
    only when its argument resolves (`demand_application`), since a caller
    may have removed the one it would forward: a `comptime if` thunk in a
    struct method called from a sibling that reads no struct parameter
    (`assets/ok/comptime_if_method_struct_parameter_unread.mojo`). That
    tolerance goes once the generator drops the parameter.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1263 A call keeps the operand of an argument its callee removed,
  which no instruction defines, where `RemoveUnusedParams` rewrites the
  call**

  The pre-elaboration view (`mir::with_removed_argument_forwards_erased`)
  erases what computed such an operand but keeps its register in the call,
  so the elaborator binds the call against the callee's full declaration
  before `drop_removed_arguments` drops it per instance.
  - A body's walk that maps registers must count such a register as the
    body's own: the unroller renumbers it per clone
    (`FrameTables::dangling`, `mono/unroll.rs`), and `materialize_body`
    sheds its type (`shed_dropped_operand_types`). Upstream's `IRMapping`
    clone needs no such case.
  - Probes: `assets/ok/comptime_for_nested_unused_argument_instances.mojo`
    and `value_parameter_inferred_from_argument_type.mojo` run only with
    the unroller's `dangling` set.
  - The fix rewrites each direct call of a reduced generator to name it
    and pass no operand for the removed argument, as upstream's pass
    rewrites the caller.
  - Found while landing R880 (2026-10-21).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1953 A call through a struct's own callable pack element is
  not resolved**

  `Self.fs[1]()` over `struct R[*fs: def() thin -> None]` reports
  "Undefined variable 'Self'", where the pin calls the bound function.
  - The call parses as an `Invoke` whose callee is the `Self.fs` member
    with `[1]` as its bracket arguments; `infer_pack_element_call`
    (`checker/comptime_validation.rs`) re-reads only a named call
    (`fs[i]()`) as the invocation of the pack's element.
  - Upstream reads the brackets on any value as its subscript, so the fix
    is the same reading for a member callee whose object is a value pack
    (`value_pack_named`), not a second rule.
  - Probe: `R[hi, bye].go()` with `Self.fs[1]()` and `print(len(Self.fs))`
    in a `@staticmethod def go()` prints `bye` and `2` at the pin.
  - Found while landing R1910 (2026-11-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1495 A refined type parameter is not a type**

  Upstream rebinds a value of type `T`, in a scope whose `where` clauses
  or `comptime if` conditions add traits, to `downcast(T, Traits)`
  (`maybeEmitRefinementRebind`, `ExprNodes.cpp`), which converts back to
  `T` at no cost (`CastRemover`, `ExprConversions.cpp`). Mojito answers
  the refinement per query (`Checker::refined_bound`) and keeps `T`.
  - The lever, checker half: a refined `TyKind::Param` carrying its
    binder's merged bound, distinct from the binder yet coercing to it
    for free. A declref, field read, call result, or subscript of type `T`
    in a refined scope takes it, as upstream's rebinds do.
  - Member lookups then read the receiver's type, and
    `Checker::refined_bound` shrinks to building that type.
  - Risk: `Ty` identity and digest, and conformance caches keyed on
    scope-dependent answers.
  - Probe: a diagnostic over a refined value. The pin's chained-comparison
    rejection under `where conforms_to(T, Comparable)` on `T: Copyable`
    spells the type 'T(Comparable & Copyable)', Mojito 'T'.
  - The refined type stays inside the check: lowering spells the binder
    until R1496.
  - Found while landing R1365 (2026-10-27).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1496 A call through a refined bound records the bound beside its
  receiver instead of in the receiver's type**

  `call.method`, `binary`, and `subscript_call` carry `refined_bound` (MIR
  schema 1.114) so `mir::Callee::resolve` finds a requirement only a
  `where` refinement or a projection's declaring trait states. Upstream
  reads it off the receiver's `downcast` type.
  - The lever: lower R1495's refined type into MIR register and place
    types, so `Callee::resolve` reads the receiver type alone, and the
    `refined_bound` fields, `MirRefinedBound`, and
    `CheckedCallContract::refined_bound` go.
  - A projection receiver (`TyKind::Assoc`) resolves through its declaring
    trait's bound the same way.
  - `MirDeclarations::copy_is_identity` then sees a refined
    `TrivialRegisterPassable` too.
  - The pack-element assumption scan in `comptime_validation.rs` folds
    into the refined type.
  - Needs a MIR text and package format bump.
  - Found while landing R1365 (2026-10-27).
  - Depends on R1495.
  - Model: Fable, Not Planned.

### Grow The CPU Standard Library *(demand-first)*

Track: `stdlib`.

The standard-library surface grows on demand toward the audited head: the
tuple, slice, optional/variant, and String surfaces, and the filesystem and
I/O residues left behind the landed files, streams, paths, and tempfile stage.
`docs/features.md` records what lands. Each entry below is a conscious,
recorded limit, listed in impact order: soundness of the executable oracle
first, then everyday spellings that reject today, then parity details. A
residue found inside a task moves to the task that owns its fix.

- [ ] **R1839 `Set` lacks upstream's `List` initializer, so
  `Set[Int]([1, 2, 1])` has one element**

  Problem: `len(Set[Int]([1, 2, 1]))` prints `1`, where the pin prints
  `2`, because `stdlib/std/collections/set.mojo` declares no
  `__init__(out self, elements: List[Self.T])` and the call binds the
  variadic initializer with the list converted to one element.
  - Port upstream's overload, with its `where conforms_to(Self.T,
    Copyable) and conforms_to(Self.T, Deinitable)` clause.
  - Probe: `len(Set[Bool]([True, False, True]))` fails with "invalid call
    to 'len': no overload matches", where the pin prints `2`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1690 `StringSpan` declares no ordering comparisons**

  `StaticString("a") < StaticString("b")` fails to check with "operator
  '<' is not defined for StringSpan and StringSpan", where the pin prints
  `True`.
  - Upstream's `string_span.mojo` declares `__lt__`, `__gt__`, and
    `__le__` over a `StringSpan` and over a `String`. Mojito's `StringSpan`
    (`stdlib/std/string.mojo`) declares only `__eq__` and `__ne__`.
  - Porting those dunders from upstream's file is the fix.
  - Probe: `a < s < c` over `StaticString` ends and a `String` middle,
    which the pin prints `True` for, fails the same way.
  - Found while landing R1684 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1679 `Codepoint`, `StringSpan`, and `Span` live outside
  upstream's modules**

  `reflect[Codepoint].name()` prints `std.string.Codepoint`, where the pin
  prints `std.collections.string.codepoint.Codepoint`: a name is qualified
  by its declaring module, and Mojito bundles these in `std/string.mojo`
  and `std/span.mojo`.
  - Upstream declares `Codepoint` in
    `std/collections/string/codepoint.mojo`, `StringSpan` in
    `std/collections/string/string_span.mojo` (where Mojito already keeps
    `get_static_string`), and `Span` in `std/collections/span.mojo`.
  - Moving them updates `implicit_public_identity` (`mojito-module`) and
    every import of the old modules.
  - Probe: `reflect[StaticString].name()` prints `std.string.StringSpan[…]`
    and a `Span`'s `std.span.Span[…]` (their parameter lists are R1680's).
  - Found while landing R1669 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1605 The `Iterable` overloads of `any` and `all` are missing**

  `any(xs)` and `all(xs)` over a `List[Bool]` or a `List[String]` reach
  only the `SIMD` overload, which reports "cannot infer type parameter
  '$dtype_value'", where the pin reduces the elements' truthiness.
  - Bundle upstream's two `Iterable` overloads from
    `std/builtin/bool.mojo` beside the `SIMD` ones in
    `stdlib/std/builtin/bool.mojo`.
  - Their clause, `where conforms_to(IterableType.IteratorType[
    origin_of(iterable)].Element, Boolable & Deinitable)`, takes a
    projection subject (R792), and it reads the value parameter
    `iterable` through `origin_of`, which a `where` clause must resolve
    the way R1508's signature scope resolves an earlier argument.
  - The body's `if item:` needs that clause's fact to refine the loop
    item, whose type is the iterator's `Element` only once R178 lands.
  - Probe: `any([True, False])`, `all([True, False])`, `any(List[Int]())`,
    `all(List[Int]())`, `any(["a", "", "b"])`, and `all([1, 2, 3])`
    print `True False False True True True` at the pin.
  - Found while landing R1568 (2026-10-28).
  - Depends on R792, R178.
  - Model: Opus, Not Planned.

- [ ] **R1421 `Optional` stores its value in a heap allocation, where
  upstream's holds it inline**

  Mojito's `Optional[T]` is an `UnsafePointer[T]` to one heap slot plus a
  size, allocated even when empty, where upstream's is a
  `Variant[_NoneType, T]` held inline.
  - Every `Optional` costs an allocation and a free, and a struct holding
    one is memory-only with a destructor.
  - Probe: each `xs[a:b]` or `s[byte=a:b]` builds a `ContiguousSlice`,
    whose two `Optional[Int]` bounds allocate twice per subscript
    (`MOJITO_RT_DBG_POINTERS` counts them natively).
  - Probe: a literal bound to a `Slice` parameter, and every `Slice(...)`
    or `slice(...)`, allocates three `Optional[Int]` cells.
  - The lever: hold the payload in the bundled `Variant` storage, as
    upstream's `_type` does.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1121 `Tuple` elements need not be `Movable`**

  Problem: the bundled `Tuple[*Ts: Movable]` rejects a non-movable
  element, where the pin's `Tuple[*Ts: AnyType]` is `Movable` only when
  every element is (`Movable where Ts.all_conforms_to[Movable]()`), and
  likewise `Deinitable`, `Defaultable`, `RegisterPassable`, and
  `ImplicitlyCopyable`.
  - The pack storage moves elements in, so the consuming constructors and
    `reverse`/`concat` need a `Movable` gate of their own.
  - The explicit application `Tuple[*Ts](*args^)` regressed at the pin
    (probe `conformance/probes/tuple_explicit_pack_application.mojo`);
    the fixtures were respelled to `Tuple(*args^)`, and Mojito keeps
    accepting both.
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
  - Depends on R2024.
  - Model: Fable, Not Planned.

- [ ] **R1122 `Tuple[Int, Int](init_with=mk)` rejects a generic `def`
  whose result type is not written dependently**

  `def mk[i: Int]() -> Int` passed as `init_with` is rejected ("declared
  callable contract 'def[i: Int]() thin -> Int' is incompatible"), where
  the pin prints `0 10`.
  - The `init_with` initializer itself exists, with upstream's
    `F: def[idx: Int]() -> Self.Ts[idx]` bound; the callable contract
    check does not close `Self.Ts[idx]` at each index before comparing.
  - Non-movable elements wait on R1121.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1123 `Tuple.first_of[T]()` returns the first element of type `T`**

  Problem: `t.first_of[String]() += "!"` is rejected ("no attribute
  'first_of'"), where the pin resolves the lookup at compile time and
  returns a reference, erroring when no element has that type.
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1125 `Hasher` drops `__init__` and `finish` reads `self`**

  Problem: the bundled `Hasher` requires `__init__(out self)` and
  `finish(var self) -> UInt64`, where the pin's trait requires neither:
  `finish(self)` borrows, construction comes from `Defaultable`, and
  `Dict`, `Set`, and `StringDict` bound their hasher
  `H: Hasher & Defaultable & Deinitable`.
  - `stdlib/std/hash/hasher.mojo` states the requirement set;
    `_ahash.mojo`, `_fnv1a.mojo`, and the user hashers in
    `assets/ok/hashable_hasher_protocol.mojo`,
    `hasher_upstream_spellings.mojo`, and `hasher_user_conformer.mojo`
    spell `finish(var self)`.
  - Probe: `hasher_user_conformer.mojo` with `Defaultable` dropped from
    `SumHasher(Defaultable, Hasher)` passes the check, which lets `Hasher`
    stand for `Defaultable` (`Checker::bound_default_constructs`, through
    its `__init__` requirement) under the bundled
    `hash[HasherType: Hasher]`, where the pin rejects it. Since R1283
    names the `Defaultable` witness at `HasherType()`, it fails at
    elaboration ("constructs the type `SumHasher` … after
    monomorphization") rather than in the check.
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1126 `Array` slices into a `Span`**

  Problem: `arr[1:3]` on an `Array[Int, 4]` is rejected, where the pin
  returns a `Span` over the selected elements whose mutability follows the
  array reference.
  - The bundled `Array` has element subscripts only; `List` already slices
    into a `Span` through `ContiguousSlice`.
  - Probe: a list display's natural type is `Array`, so a starred target
    over one (`var [y1, *mid, y2] = [7, 8]`) is rejected ("invalid call to
    '__getitem__'"), where the pin prints `7 8 0` for `print(y1, y2,
    len(mid))`. So is `var xs = [7, 8]; var s = xs[1:]`. Found while
    landing R1137.
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1127 `RawFileDescriptor` holds a `c_int` with `-1` as its niche**

  Problem: the bundled `RawFileDescriptor` keeps `_value: Int`, an
  `__init__(value: Int = 1)` that accepts any integer, and no
  `unsafe_unchecked_value=` constructor, where the pin's takes `c_int`,
  aborts on a negative value, exposes `-1` through
  `UnsafeSingleNicheable` so `Optional[RawFileDescriptor]` is one word,
  and spells `stdout = RawFileDescriptor(unsafe_unchecked_value=1)`.
  - The 2026-10-17 re-pin renamed the struct from `FileDescriptor` and the
    field from `value`; the type and the niche are this entry.
  - The VM's `descriptor_value` and the Pliron `print` lowering read
    `_value` as an `Int`; a `c_int` reaches them as a `SIMD` scalar.
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R669 The bundled `Iterator` trait is not upstream's**

  Problem: `it.bounds()` on an `Iterator` is rejected ("has no method
  'bounds'"), where the pin answers it.
  - Upstream declares `trait Iterator(Deinitable, Movable)` with a defaulted
    `def bounds(self) -> Tuple[Int, Optional[Int]]` returning `(0, None)`.
  - The bundled trait (`stdlib/std/iter.mojo`) refines nothing and has no
    `bounds`.
  - Every bundled conformer must still conform once the refinement lands.
  - The bundled `List.__init__` from an iterable starts at the default
    capacity, where upstream's sizes it by `iter(iterable).bounds()`
    (with R670).
  - Found while landing R665 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R670 The free `iter()` function is missing**

  Problem: `var it = iter(xs)` is rejected with "Undefined variable
  'iter'", where the pin runs it.
  - Upstream declares a borrowed overload,
    `iter[IterableType: Iterable](ref iterable: IterableType) ->
    IterableType.IteratorType[origin_of(iterable)]`, and an owned one over
    `Some[IterableOwned]`.
  - The borrowed overload's body is `return iterable.__iter__()`, which
    checks since R665 (`assets/ok/generic_iterable_explicit_iter.mojo`).
  - Add both to `stdlib/std/iter.mojo` beside `next`.
  - `List.__init__` from an iterable reads `iter(iterable).bounds()`
    upstream (R669).
  - Found while landing R665 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1737 `rebind_var` is undeclared**

  Problem: `rebind_var[type_of(self).T](value^)` is rejected ("Undefined
  variable 'rebind_var'"), where the pin moves an owned value into the
  type a parameter refinement names.
  - Mojito's `rebind` is a checker builtin (`checker/rebind.rs`), and the
    bundled library has no `std.builtin.rebind` module, so upstream's
    library `rebind_var` has no home.
  - Upstream's body takes `ref dest_ref = rebind[dest_type](src)`, moves
    it out with `Pointer(to=dest_ref).unsafe_take_pointee()`, and ends
    `src` with `std.memory.forget_deinit`, which is also missing.
  - Probe: the bundled `List.__init__` from an iterable copies and
    `rebind`s each element where upstream's moves it with `rebind_var`
    (with R178).
  - Found while landing R1154 (2026-10-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1229 `enumerate` and `zip` are missing**

  Problem: `for i, x in enumerate(xs)` and `for x, y in zip(xs, ys)` stop
  with "Undefined variable 'enumerate'", where the pin's `std.iter`
  (`Mojo/stdlib/std/iter/__init__.mojo`) yields tuples a loop target
  destructures.
  - They are the commonest tuple-yielding iteration.
  - Port upstream's `_Enumerate` and `_Zip2` iterators and the free
    functions over the bundled `Iterator`/`Iterable` traits.
  - Probe: `conformance/probes/enumerate_zip_iteration.mojo`.
  - Found while planning R1138 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R810 `ParameterList.reduce`, `any`, `all`, and `contains` are not
  ported**

  Problem: the pin's `ParameterList` declares these reductions, and the
  bundled one does not.
  - Upstream's `reduce` is `#kgen.param_list.reduce`. Port it through a
    compiler-private stand-in admitted only in `variadics.mojo`, as
    `__param_list_address` is, and record the stand-in in the module
    header.
  - `contains` also reads `downcast`/`rebind` over `Equatable`.
  - Found while planning R493 (2026-10-13).
  - Depends on R808 (generator parameters), R800 (`__getitem_param__`),
    and R1485 (`Self._AnySatisfiesReducer[predicate, ...]`).
  - Model: Opus, Not Planned.

- [ ] **R811 `ParameterList.tabulate`, `splat`, and `_concat` are not
  ported**

  Problem: the pin's `ParameterList` builds lists with these members, and
  the bundled one has none.
  - Upstream uses `#kgen.param_list.tabulate` and `.concat`. Port them as
    variadics-private stand-ins, like R810's.
  - Found while planning R493 (2026-10-13).
  - Depends on R807 (the raw list type), R808 (generator parameters), and
    R1485 (an unbound application, `idx=_`).
  - Model: Opus, Not Planned.

- [ ] **R812 `ParameterList.map_to_type` is not ported**

  Problem: the pin's `ParameterList.map_to_type` is missing from the
  bundled struct.
  - Its body is `TypeList.tabulate[...]`, so it needs a bundled `TypeList`
    with `tabulate`.
  - Found while planning R493 (2026-10-13).
  - Depends on R811, R1181 (`TypeList.tabulate`), and R1486 (a type
    generator applied in a type position).
  - Model: Opus, Not Planned.

- [ ] **R1181 `TypeList.tabulate`, `splat`, and `_concat` are not ported**

  Problem: the bundled `TypeList` has no list builders, where the pin's
  builds lists with these members.
  - `TypeList._concat[A.values, B.values]()` is a compiler rule today
    (`Checker::typelist_receiver`, and `comptime/rewrite.rs` for a bound
    pack).
  - Upstream uses `#kgen.param_list.tabulate` and `.concat`. Port them as
    variadics-private stand-ins, as R811 does for `ParameterList`.
  - Restore `FastMathFlag._enum_case_types` in `stdlib/std/simd.mojo` to
    upstream's `TypeList.splat[...]` spelling.
  - Found while landing R1168 (2026-10-20).
  - Depends on R808 (generator parameters), R1485 (`Self._MapTabulator[
    ToTrait, Mapper, idx=_]`), and R1486 (a type generator applied in a
    type position).
  - Model: Opus, Not Planned.

- [ ] **R813 `FormatStruct` lacks upstream's callback `fields` overload**

  Problem: `ParameterList.write_repr_to` writes its text by hand, where
  upstream calls `FormatStruct(...).params(TypeNames[Self.type]())
  .fields(write_fields)` with a capturing `def write_fields`.
  - The bundled `FormatStruct` (`std/format/_utils.mojo`) has no
    `fields(fields_fn: Some[def[T: Writer](mut T)])` overload, and
    `Some[def[...](...)]` is rejected with "unknown type 'Some'".
  - The bundled `TypeNames[*Ts: Movable]` is narrower than upstream's
    `AnyType`, so it cannot name `ParameterList`'s element type.
  - The bundled collections' `write_repr_to` bodies write by hand too.
  - `IndexList.write_repr_to` is omitted (`stdlib/std/utils/index.mojo`):
    restore it with its `import std.format._utils as fmt`.
  - Done when they and `ParameterList` use upstream's builder text.
  - Probe: `def f(g: Some[def(var Int)])` is the same "unknown type
    'Some'". `List`, `Array`, `Set`, `Dict`, and `StringDict`
    `deinit_with` and `Set`/`Dict` `clear_with` spell an explicit
    `[F: def(var Self.T)]` binder where upstream spells
    `Some[def(var Self.T)]`; respell them when it lands (found while
    landing R1433, 2026-11-03).
  - Found while landing R493 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R343 `Tuple.reverse` and `Tuple.concat` move each element by a
  transfer out of the private storage where upstream moves it through a
  pointer**

  Problem: both are declared in `std/builtin/tuple.mojo` with upstream's
  signatures, but their bodies write
  `Pointer(to=result[i]).unsafe_write(Pointer(to=self[k]).unsafe_take_pointee())`,
  where upstream
  writes `Pointer(to=result[i]).unsafe_write_move_from(rebind[Pointer[
  type_of(result[i]), origin_of(self)]](Pointer(to=self[k])))`.
  - `concat` retypes the destination place instead of the source pointer:
    `rebind[Self.Ts[i]](result[i])`.
  - `concat` reads its length as `Self.Ts.length` where upstream calls
    `Self.__len__()`.
  - The loop headers read `Self.Ts.length` and `OtherTs.length` where
    upstream reads `type_of(result).__len__()` and
    `type_of(other).__len__()`.
  - Pinned by `assets/ok/tuple_reverse_concat.mojo`.
  - `Pointer.unsafe_write_move_from` exists, and the storage is trivially
    destructible, so the respelling waits only on the dependencies below.
  - Depends on R1509
    (a `type_of(...)` receiver), R495 (its symbolic spelling), R340 (`Self.__len__()` in a header), and
    R344.
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

- [ ] **R214 An annotated `Optional[T]` local initialized from a bare
  struct construction fails MIR verification**

  Problem: `var o: Optional[P] = P(4)` fails MIR verification with an
  untyped register for the constructor call.
  - Workaround: spell the wrapper (`Optional[P](P(4))`, `Optional(P(4))`).
  - A literal payload (`var n: Optional[Int] = 6`) converts.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R218 `@fieldwise_init` beside a hand-written `__init__` is
  rejected**

  Problem: a struct carrying both `@fieldwise_init` and a hand-written
  `__init__` rejects.
  - Workaround: spell every constructor by hand.
  - Cause: the check resolves a fieldwise construction on its own path,
    the fieldwise tail of `Checker::infer_construction`, not by overload
    resolution over the struct's `__init__` set, where upstream declares
    the synthesized `__init__` at declaration resolution as one overload
    beside the written ones (`findFieldwiseInit` tests the conflict).
  - The synthesized `S.__init__`
    (`CheckedProgram::synthesizes_fieldwise_constructor`) is the overload
    to add; its symbol must then be overload-qualified.
  - Probe: the bundled `_VariadicListIter` writes its constructor out
    where upstream's declares `@fieldwise_init` beside one.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R219 A bound-generic def whose template body calls itself with a
  concrete argument reports its own specialization undefined**

  Problem: `makedirs(head, exist_ok=...)` inside `makedirs[PathLike]` mints
  its own specialization while checking and reports it undefined.
  - Workaround: recurse through a non-generic helper.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R221 `Pointer.unsafe_bitcast[U]()` is not accepted**

  `p.unsafe_bitcast[Int8]()` over a `Pointer[UInt8, o]` is rejected with
  "compile-time parameter arguments are not supported here", where the pin
  retypes the pointer to `Pointer[Int8, o]`.
  - Upstream's body is `pop.pointer.bitcast` over the `!kgen.pointer`.
    `pointer.convert` is the precedent for a retype that keeps the bits,
    but a new element type needs the VM's typed memory to reinterpret the
    pointee or refuse.
  - Then retype `pop.string.address` to the pin's
    `!kgen.pointer<scalar<si8>>` (`TyKind::KgenPointer` of `Int8`), respell
    `StringLiteral.ptr` and `StaticString(_kgen)` as upstream's
    `Pointer[_, ImmStaticOrigin](_mlir_value=…).unsafe_bitcast[Byte]()`,
    and drop `is_bundled_kgen_string_source`'s address gate.
  - Probe: upstream's `VariadicList` constructor ends with
    `.unsafe_bitcast[Self._EltPointerType]()` over the array's first
    element (R1317).
  - Probe: upstream's `Pointer(unsafe_from_address=)` is
    `Pointer(to=unsafe_from_address).unsafe_bitcast[type_of(self)]()[]`,
    and `gather`/`scatter` load their `SIMD[.int, size]` addresses as a
    `!kgen.simd<size, address>` through
    `Pointer(to=base).unsafe_bitcast[...]()[]` (R1520).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R222 `CStringSlice` views `Byte` elements instead of `c_char`**

  Problem: upstream's `CStringSlice` views `c_char` elements, Mojito's views
  `Byte`.
  - Upstream's type is now `CStringSpan`, built `CStringSpan(unsafe_from_ptr=
    ptr.unsafe_bitcast[c_char]())`; the bundled `CStringSlice` has only the
    `String` initializer.
  - Probe: `"ab".as_c_string_span()` and its deprecated
    `as_c_string_slice()` are unported for that reason, where the pin's
    `len(…)` prints `2` (found while landing R1655).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R223 `std.builtin.rebind.downcast` is missing, so `Dict`/`Set`
  views are guarded by `Copyable` instead**

  Problem: with no `downcast`, `Dict`/`Set` guard `keys`, `values`, `items`,
  and `__iter__` with `where conforms_to(K, Copyable)` instead of laundering
  the parameter.
  - `Dict.keys` needs copyable values as well as keys, since the key view
    wraps the entry view.
  - The bundled `ParameterList.__iter__` omits upstream's
    `rebind[_MLIR.KGENParamListType[downcast[Self.type, Copyable]]]`
    laundering of `Self.values` and is gated `where conforms_to(Self.type,
    Copyable)`.
  - `List.__iter__(ref self)` carries `where conforms_to(Self.T,
    Copyable)`, where the pin's asserts the bound with `comptime assert`
    and `downcast`, so `List.__hash__` cannot use the pin's `for element
    in self` body and indexes through `unsafe_get` instead.
  - Probe: a `comptime for` over a display of non-`Copyable` elements
    (`assets/type_error/comptime_for_display_not_copyable.mojo`, merged
    from R903) is rejected as "'_ArrayOwnedIter[P]' ... does not conform to
    trait 'Copyable'", where the pin reports "Array iteration requires the
    element to be `Copyable`".
  - Its cause: `Array.__iter__(ref self)` is gated `where
    conforms_to(Self.T, Copyable)`, so the display selects `__iter__(var
    self)`. The pin's asserts the bound with `comptime assert`, and its
    `IteratorType` is `_ArrayIter[downcast[Self.T, Copyable], ...]`;
    without the downcast, the `comptime for` bound rejects `_ArrayIter[P,
    2]` as not an `Iterator`.
  - Probe: the bundled `List`, `Set`, `Array`, `Optional`, `StringDict`,
    and `Dict` declare `Iterable where conforms_to(T, Copyable)` (`Dict`
    over `K` and `V`), where upstream's declare it unconditionally, so
    `conforms_to(List[P], Iterable)` over a non-`Copyable` `P` is `True` at
    the pin and `False` here.
  - Probe: the bundled `_ListIter`, `_ArrayIter`, `_SpanIter`,
    `_DictKeyIter`, `_DictValueIter`, `_DictEntryIter`, and `_OptionalIter`
    declare `Iterator where conforms_to(T, Copyable)`, where upstream's
    take a `Copyable` element and are unconditionally `ImplicitlyCopyable`,
    `Iterable`, and `Iterator`.
  - Probe: `List(d.keys())` over a `Dict[String, Int]` matches no
    `List` constructor ("'_DictKeyIter[...]' ... does not conform to
    trait 'Iterable'"), where the pin copies the keys.
  - An unconditional `Iterable` also needs R178: the bundled `Iterable`
    requires `comptime Element: Copyable & Movable`.
  - Probe: `downcast[Self.Plugins._get_type_at_index[idx], Self.Trait]`,
    the `#kgen.downcast<: type_of(T) +T> : _Trait` attribute, is "outside
    the subset", which `TargetPluginSelector.selected` reads (R1694).
  - Depends on R178.
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

  Problem: the I/O stage left `isatty` (`RawFileDescriptor.isatty` / `fchdir`)
  unported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R230 A nullable pointer result is a raw address, where upstream's
  is an `Optional[Pointer[T, origin]]`**

  The bundled `getenv` and `Path` helpers test a null result as
  `Int(ptr) == 0`, where upstream returns an `Optional` of the pointer.
  - Upstream deprecated the `OptionalPointer` aliases at
    `1.3.0.dev2026110105`, so port the `Optional[Pointer[...]]` spelling,
    not the alias.
  - `stdlib/std/os/env.mojo` and `stdlib/std/pathlib/path.mojo` name the
    sites.
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
  - Probe: the bundled `struct DType` omits `KeyElement` from its
    conformance list (`stdlib/std/builtin/dtype.mojo`), which "unknown trait
    'KeyElement' in a type-parameter bound" rejects; restore it there.
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
  - Since `1.3.0.dev2026110105` the error-taking form also takes `var
    self` and `deinit self` (R1820).
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

- [ ] **R144 Time, random, and testing slices**

  Goal: deterministic testable cores, with host-dependent behavior behind
  runtime services.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R427 `ord` is undefined**

  Problem: `ord("{")` returns `123` at the pin, while Mojito reports
  "Undefined variable 'ord'".
  - Upstream's `ord` lives in `builtin/string_literal.mojo` beside `chr`.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R428 `NoneType` is not a type name**

  Problem: `Variant[StringSlice, Int, NoneType, Bool]` builds at the pin,
  while Mojito reports "Undefined variable 'NoneType'".
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R431 `Span` has no `unsafe_get`**

  Problem: `bytes.unsafe_get(i)` over a `Span[UInt8]` reads the element at
  the pin, while Mojito reports "type 'Span[UInt8]' has no method
  'unsafe_get'".
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R699 A `Dict` destroys its entries in insertion order, where the
  pin's table destroys them in slot order**

  Problem: `{mk(5): mk(6), mk(8): mk(9)}` ends with `del 25 del 26 del 28
  del 29` in Mojito and `del 28 del 29 del 25 del 26` at the pin.
  - Mojito's bundled `Dict` keeps its entries in one `List`
    (`stdlib/std/collections/dict.mojo`), so teardown follows insertion.
  - Upstream's table destroys entries in slot order.
  - `Int` keys happened to match insertion order in every probe.
  - Found while landing R694 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1417 An empty `List` starts at capacity 4, where the pin's starts
  at 0**

  The bundled `List()` allocates four slots and doubles, where upstream's
  starts with dangling storage and grows to `capacity * 2 | (capacity ==
  0)`: 1, 2, 4, and so on.
  - An element type printing its move initializer shows it: three appends
    print each element's `move` once in Mojito, where the pin also prints
    `move 1` at the second append and `move 1`, `move 2` at the third.
  - `capacity()` differs too.
  - Lever: `__init__(out self)` and `grow` in
    `stdlib/std/collections/list.mojo`.
  - Found while planning R136 (2026-10-25).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1411 `std.iter.map` is not ported**

  `from std.iter import map` fails with "module 'std.iter' has no
  declaration named 'map'", where the pin's `map[Token.integer]([1, 2, 3])`
  maps a list through a static method.
  - Probe: the pin's enum integration test's `test_function_value`, `for
    token in map[Token.integer]([1, 2, 3])`.
  - Found while landing `enum` (2026-10-25).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1455 The bundle has no `std.python`, so `String` is not
  `ConvertibleFromPython`**

  Problem: upstream's `String` declares `ConvertibleFromPython`, a
  `std.python` trait, which the bundled `String` cannot name.
  - The trait converts from a `PythonObject`, and Mojito has no Python
    interop.
  - For the same reason the bundled `StringLiteral` omits upstream's
    `ConvertibleToPython` conformance, `ConversionToPythonErrorType`, and
    `to_python_object` (found while landing R1655).
  - If Python interop is out of scope, this moves to `docs/non-goals.md`.
  - Found while landing R578 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1985 The bundled `List` declares a `__setitem__` upstream's does
  not, so a store at an `Indexer` index is rejected**

  `l[Int32(0)] = 7` over a `List[Int]` is rejected ("no overload matches
  the supplied arguments"), where the pin stores through the reference
  `__getitem__(ref self, idx: Some[Indexer])` returns.
  - Upstream's `List` has no `__setitem__`; the bundled one keeps an
    `Int`-only `__setitem__` (`stdlib/std/collections/list.mojo`), which
    every subscript store selects.
  - Dropping it, as upstream, runs `l[Int32(0)] = 7` and every `Int` store
    through the getter's reference, but two gaps then show.
  - The replaced element is never destroyed (R1216):
    `assets/ok/subscript_write_back_lifecycle.mojo` loses its `del` lines.
  - A store through `p[][0]` over a `Pointer` of parametric mutability is
    accepted (R1986): six `assets/type_error/*pointer*write*` fixtures
    then report "immutable reference returned by '__getitem__'" instead,
    and `pointer_param_symbolic_origin_write.mojo` and
    `pointer_field_symbolic_generic_body_write.mojo` run.
  - When it lands, drop the `List.__setitem__` row from
    `seeded_transfer_effects` (`checker/overload_support.rs`) and the
    sentence on it in `docs/features.md`'s argument-exclusivity row.
  - Found while landing R1564 (2026-11-07).
  - Depends on R1216, R1986.
  - Model: Opus, Not Planned.

- [ ] **R1663 `SIMD.write_padded` is missing**

  `SIMD[DType.int32, 4](1, -22, 333, 4).write_padded(t, 4)` stops with
  "type 'SIMD[DType.int32, 4]' has no method 'write_padded'", where the pin
  writes `[   1, -22, 333,   4]`.
  - Upstream's body reads `_calc_initial_buffer_size` from
    `collections/string/string.mojo`, over a compile-time `Array[Int, 32]`
    table and `is_32bit`, and calls `abs` over a `Scalar[dtype]`.
  - Port the helpers into `stdlib/std/string.mojo`, then the member into
    `stdlib/std/simd.mojo` beside `write_to`.
  - Depends on R1458, R1661, and R1717.
  - Model: Opus, Not Planned.

- [ ] **R1775 `std.utils.lock` is not bundled**

  `from std.utils import BlockingSpinLock` fails, where upstream's
  `std/utils/lock.mojo` declares `BlockingScopedLock`, `BlockingSpinLock`,
  and `SpinWaiter`, and `std/utils/__init__.mojo` re-exports them.
  - Port the file into `stdlib/std/utils/lock.mojo` in upstream's order and
    restore the re-export the bundled `std/utils/__init__.mojo` omits.
  - Probe its atomics and `sleep` reads first: header-list what is not
    bundled with the entries that own each.
  - Found while landing R1674 (2026-10-31).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1703 `std.utils.coord.Coord` is not bundled**

  `from std.utils.coord import Coord` fails, where upstream's
  `std/utils/coord.mojo` declares it.
  - Port the struct and what `_ReduceGeneratorPluginHookFnType` (R1702)
    reaches of it.
  - Header-list the rest of the 1977-line file with the entries that own
    each part.
  - Depends on R1674.
  - Model: Opus, Planned.

- [ ] **R1758 `std.sys.info.platform_map` is not ported**

  `comptime EDEADLK = platform_map["EDEADLK", linux=35, macos=11]()`
  fails, where the pin's value is `35` on Linux.
  - Its `else` arm ends in the `-> Never` call
    `CompilationTarget.unsupported_target_error[operation=operation]()`,
    which Mojito reports as "does not return a value on every path"
    (R50).
  - Its string-literal `operation` argument binds an `Optional[String]`
    parameter, which needs R1864.
  - Probe `platform_type_map` (`downcast`, a type-level conditional,
    `_UnsupportedTarget`) in the same session, and split it off if it
    needs more.
  - Depends on R50, R1651, and R1864.
  - Model: Opus, Not Planned.

- [ ] **R1759 `std.sys.info`'s `Endian`, `is_little_endian`, and
  `is_big_endian` are not ported**

  `from std.sys.info import Endian` fails, where the pin prints `True` for
  `Endian.native() == .little`.
  - `Endian` declares `EnumLike` over `TypeList.splat` (R1181).
  - `KgenTarget::field` (`crates/mojito-types/src/ct.rs`) does not answer
    `target_get_field`'s `endianness` field. Add it, read from the data
    layout's leading `e`/`E`, together with `reads_field`.
  - Restore the `std.sys` re-exports of all three.
  - Depends on R1181.
  - Model: Opus, Not Planned.

- [ ] **R1760 `std.math` has no `exp2` or `log2`**

  `from std.math import exp2, log2` fails, where upstream's `math.mojo`
  declares both.
  - `exp`'s `is_gpu()` arm returns `exp2(x * log2e)`, and `log`'s
    `is_nvidia_gpu()` arm calls `log2`. With the target predicates ported,
    those arms are omitted only for the missing functions.
  - Port both verbatim, then restore the two arms. A CPU target takes
    neither, so `assets/ok/math_exp.mojo` and `assets/ok/math_log.mojo`
    keep their output.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1824 `std.io` has no owning `FileDescriptor`, and `FileHandle` is
  `Defaultable` with a non-consuming `close`**

  At `1.3.0.dev2026110105` upstream's descriptor surface is three types,
  where the bundled `std.io` has only `RawFileDescriptor`.
  - `FileDescriptor` owns a descriptor and is linear: `fd^.close()`
    raises if the close fails, and `FileDescriptor(copy_borrowed=)`
    duplicates one with close-on-exec.
  - `BorrowedFileDescriptor` is a copyable view carrying its owner's
    origin.
  - `RawFileDescriptor` gains a raising `close()`, and the descriptor
    reads, `fchdir`, and closes raise `IOError`.
  - `FileHandle` owns a `FileDescriptor`, is no longer `Defaultable`, and
    its `close()` consumes it. `NamedTemporaryFile` follows.
  - Port `stdlib/std/io/file_descriptor.mojo`, `file.mojo`, and
    `stdlib/std/tempfile/` from the pinned worktree, then the fixtures that
    call `close()` on a `mut` handle.
  - `assets/ok/file_handle_roundtrip.mojo` and
    `assets/ok/file_descriptor_stdout_order.mojo` are `ledgered` in
    `conformance/assets-mojo-rejects.tsv`, and
    `assets/runtime_error/file_read_closed_handle.mojo` in
    `conformance/assets-mojo-errors.tsv`, until this lands.
  - Ledger name: `file-handle-roundtrip`, `file-descriptor-stdout-order`.
  - Found at the 2026-11-01 re-pin.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1825 `OwnedPointer` has no in-place `init_with` initializer or
  `deinit_with`**

  At `1.3.0.dev2026110105` `OwnedPointer(init_with=)` constructs its value
  in place, so the element need not be `Movable`, and `deinit_with()`
  destroys one whose element is not `Deinitable`.
  - The initializers that copy a value into the allocation are deprecated
    upstream.
  - Port both from the pinned `std/memory/owned_pointer.mojo`.
  - Probe: the bundled `other=` initializer copies through
    `Self(copy_value=other[])`, where the pin's body is `self = {init_with
    = lambda () -> Self.T: other[].copy()}`; respell it once `init_with=`
    lands.
  - Found at the 2026-11-01 re-pin.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1826 `Layout` names its element count `count`, where the pin's is
  `length` and `Layout` is `Sized`**

  `Layout[T](count=n)` and `layout.count()` are deprecated at
  `1.3.0.dev2026110105`. Upstream constructs one with `Layout[T](length=n)`
  and reads it back with `len(layout)`, and `ManagedAllocation` is `Sized`
  too.
  - Port the pinned `std/memory/alloc.mojo` surface: the `length=`
    initializer, `__len__`, and the `Sized` conformances, keeping `count=`
    as upstream's deprecated spelling.
  - Respell the corpus's `count=` uses.
  - Found at the 2026-11-01 re-pin.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1828 The plugin hooks name their math ops by string, where upstream's
  are `__enum` cases**

  `CurrentPlugin.unary_math_fn["sqrt", dtype, width]` and
  `binary_math_fn["pow", ...]` take a `StringLiteral` op, where
  `1.3.0.dev2026110105` declares `__enum _UnaryMathOp` and `__enum
  _BinaryMathOp` in `std.math` and spells the call `unary_math_fn[.sqrt,
  ...]`.
  - Port both enums and the hook signatures in `stdlib/std/_plugin/` and
    `stdlib/std/math/math.mojo`, and respell every call site.
  - `assets/ok/std_plugin_hooks.mojo` and
    `assets/ok/dtype_value_member_argument.mojo` are `ledgered` in
    `conformance/assets-mojo-rejects.tsv` until this lands.
  - Found at the 2026-11-01 re-pin.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1978 `get_defined_dtype` is not bundled**

  `std.sys.defines.get_defined_dtype` is missing, since its body reads
  `DType._from_str`, which the bundled `DType` does not declare.
  - Port `DType._from_str` from upstream's `std/builtin/dtype.mojo`, then
    `get_defined_dtype` verbatim into `stdlib/std/sys/defines.mojo`, and
    re-export it from `std.sys`.
  - Probe: `from std.sys import get_defined_dtype` then
    `print(get_defined_dtype["DT", DType.int8]())` prints `int8` at the pin.
  - Found while landing R1598 (2026-11-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1982 `MojoVersion` and `MOJO_VERSION` are not bundled**

  `std.sys.defines.MojoVersion` reads the compiler's version through the
  `lit.mojo.version.major`/`minor`/`patch` ops, which the checker does not
  admit.
  - Admit the three ops as constant `index` values of the pin's version,
    then port `MojoVersion` and `MOJO_VERSION` verbatim.
  - Probe: `from std.sys.defines import MOJO_VERSION` then
    `print(MOJO_VERSION.major)` prints the pin's major version.
  - Found while landing R1598 (2026-11-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R2011 The bundled `Error` omits upstream's `StackTrace` and its
  `_stack_trace` field**

  Upstream's `Error` holds `_stack_trace: Optional[StackTrace]` and
  answers `get_stack_trace()`. Mojito's holds only its `_error` message.
  - `StackTrace.collect_if_enabled` calls the compiler runtime's
    `KGEN_CompilerRT_GetStackTrace`, which `mojito-runtime` does not
    export, and holds an `ArcPointer[OwnedPointer[UInt8]]`.
  - The field changes the native `MjError` layout the runtime ABI fixes
    to one `MjString` (`docs/native-abi.md`, `rt_abi.rs`).
  - The `depth` keyword of `Error.__init__(String)` is kept and ignored.
  - Probe: `reflect[Error].field_count()` is `1`, where the pin's is `2`.
  - Probe: `e.get_stack_trace()` is "has no method", where the pin answers
    `None` with tracing off.
  - Found while landing R1938 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Packaging, Artifacts, And Developer Tooling

Track: `tooling`.

- [ ] **R2013 The MIR pass-test inputs spell the removed `DType` and
  `Error` type tags, so `tests/mir_pass_test.rs` cannot load them**

  Schema 1.177 (R1936) and 1.178 (R1938) made both types library structs,
  but 143 `tests/mir_passes/*/*.mir` cases still write `DType` and 26
  write `Error` as bare type tags. `opt` rejects each at load.
  - Respell each as `struct_type { name: DType, arguments: [] }` or
    `struct_type { name: Error, arguments: [] }`, adding the struct rows
    the verifier needs, or re-emit the case as `tests/mir_passes/README.md`
    describes.
  - The `Error` cases also call the retired builtin `func: Error`. Replace
    it with a call of the library initializer, or re-emit.
  - Probe: `mojito opt --pass mem-2-reg
    tests/mir_passes/mem-2-reg/try_region.mir` reports "unknown type
    `DType`".
  - Found while landing R1938 (2026-11-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1954 An older artifact's parameterized associated type keeps
  its one-binder pack arguments**

  The reader regroups an older artifact's arguments around a pack's two
  binders only where the application names its target (R1911), and an
  `assoc` record names no owner whose binders its arguments align with.
  - Probe: a 1.158 artifact holding `assoc { base, member, arguments }`
    for a parameterized associated type declared `[*vs: Int]` reads its
    arguments one short of the declared binders.
  - Also: the visible `TypeList` of an older type pack takes the trait
    set of its bounds without the checker's implication pruning
    (`canonical_trait_set`), which the reader cannot consult.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1297 The MIR text reader hangs on an unbalanced type**

  `mojito opt` on an artifact whose register type reads
  `variadic_pack(struct_type { name: Token) }` (a `)` where the record's
  `}` belongs) never returns, where it should report the malformed type.
  - Reproduce by truncating a `reg_type` in any
    `tests/mir_passes/*/*.mir` case that way and running `mojito opt` on it.
  - Where it loops is not yet located.
  - The type grammar is read in
    `crates/mojito-mir/src/mir/text/parse/types.rs`.
  - Done when that input fails with a located diagnostic, pinned as an
    error case in `tests/mir_passes/`.
  - Found while landing R1102 (2026-10-23).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R530 `benchmarks/compile/tuple.mojo` is not valid Mojo**

  Problem: the pin rejects the benchmark (`p[0]` against `__getitem__[i: Int](self)`, and a non-`Deinitable` field), and Mojito now rejects it too.
  - `compiler_test::discovery_scan_matches_the_checked_arena` dropped it from its list.
  - Rewrite it in the pin's spelling (`__getitem_param__`, a `Deinitable` bound) and restore it.
  - Depends on nothing.
  - Model: Opus, Planned.

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
  - `explicit_destroy` re-derives deinitability per struct per pass.
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

- [ ] **R587 `--timings` drops every span the elaborator thread records**

  `mojito --timings run` reports no `package.body.*` span, and no other
  span recorded while elaborating, because the elaborator runs on its own
  thread and timing events are per-thread.
  - Timing events are `thread_local!` (`crates/mojito-common/src/timing.rs`).
  - `native::mono` elaborates on a scoped thread named `elaborator`
    (`crates/mojito-native/src/native/mono.rs`).
  - Only counts recorded afterwards on the main thread survive, such as
    `package_bodies_decoded`.
  - `emit-mir` shows the spans because it decodes on the main thread.
  - Found while landing R584 (2026-10-10).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1981 The CLI takes no `-D NAME[=VALUE]` define**

  `mojo run -D ASSERT=all f.mojo` enables every `assert`, where Mojito has
  no `-D` and every `#kgen.param.expr<get_env>` sees an empty table.
  - Add `-D`/`--define` to `src/main.rs` and `Compiler`, and fill
    `mojito_types::param_expr::fold::compile_time_define` from it, for the
    checker's folds and the elaborator's per-instance ones alike.
  - Then `-D ASSERT=all` makes `assert n == 3, "boom"` fail, and
    `-D ASSERT=bogus` fails `_assert_enabled`'s instance, as at the pin.
  - Revisit `docs/non-goals.md`'s note on evaluating stdlib asserts as the
    pin does under `-D ASSERT=all`.
  - Found while landing R1598 (2026-11-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R157 Distribution reproducibility gate** *(last)*

  Goal: the release check rebuilds, tests, documents, and reproduces
  conformance from the crates.io archive alone.
  - It stays last whatever else moves.
  - Depends on every other entry in this track.
  - Model: Fable, Planned.

- [ ] **R439 The VM runs `format` about fifteen times slower than the Rust
  formatter it replaced**

  Problem: 10,000 `"{} and {}".format(i, "x")` calls take 35.6 s in a
  release build, against 2.3 s before `format` became the bundled Mojo
  port.
  - Each call parses the template in interpreted Mojo; an
    `Optional[Int]` round trip alone costs about 0.13 ms in the VM.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

### Code Organization Follow-Ups *(behavior-preserving)*

Track: `code-org`.

The module splits (`docs/symbol-map.md`) removed every file over 3,000
lines. The rest needs semantic extraction, not line moves.

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

- [ ] **R692 An interior invalidation's `include_base_generation` has no
  producer**

  Problem: no checked fact sets `InteriorInvalidation::include_base_generation`
  since R685 stopped a `Dict` lookup from replacing its `value` generation,
  yet the flag still rides through `mojito-checked`, MIR's
  `InvalidateInteriors`, the text format, and the interior analysis.
  - Delete the flag end to end, with the MIR text minor and package format
    bumps that removing a required field needs.
  - Found while landing R685 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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
- One session's work per checkbox. An entry is a task `scripts/claude_loop.py`
  hands to one session, and the session's only way to finish it is to delete
  it. Work that needs several sessions is several checkboxes, chained by
  their Depends bullets. Never file an entry that is "not one session's
  work". A session that cannot finish its entry deletes it and files what
  remains the same way, rather than rewriting it as one larger "what
  remains" entry.
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
