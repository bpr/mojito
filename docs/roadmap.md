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

Next free ID: **R1650**.

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

- [ ] **R1480 A package's declaration residue keeps an `Elaborated` table
  no importer reads**

  A module value constant's residue holds an `ElaboratedConstant` row
  beside the check's own rows, where upstream's package holds a constant
  once, as its declaration.
  - Since R1388 the elaborator folds nothing and nothing calls
    `CheckContext::record_elaborated`, so the rows only mirror the check's
    integer constants (`elaborated_constant`), and
    `InstalledInterfaces::elaborated` has no caller.
  - The lever: delete `ElaboratedConstant`, `ElaboratedConstants`,
    `CheckContext::record_elaborated` with its field, `Table::Elaborated`
    with `DeclarationInterface::elaborated` and its `project`/`join`
    projection (`elaborated_constant`) and the interface unit tests that
    pin it, `InstalledInterfaces::elaborated`, and `declaring_modules`'
    `elaborated` parameter.
  - Bump `PACKAGE_FORMAT`.
  - Done when `rg ElaboratedConstant crates` is empty.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1642 A variadic collector is no parameter of a compile-time
  application**

  `ApplicableShape::applicable` (`checker/comptime_validation.rs`) lists
  no callable with a `*xs` collector, so a call of one denotes nothing in
  a type, where the pin keeps it as the callee's application with its
  arguments flat (`total(n, Int(1))`).
  - Probe: `f[1](SIMD[DType.int8, total(1, 1)](1))` over `def f[n:
    Int](x: SIMD[DType.int8, total(n, 1)]) -> SIMD[DType.int8, total(n,
    1)]` and `def total(*xs: Int) -> Int` stops with "not a compile-time
    Int constant: not an associated comptime expression". The pin prints
    `[1, 1]`.
  - Probe: `SIMD[DType.int8, total(n, 1)]` against `SIMD[DType.int8, n +
    1]` is rejected at the pin naming `total(n, Int(1))`; Mojito stops
    with the same "not an associated comptime expression".
  - Probe: `SIMD[DType.int8, cnt(n, True)]` over `def cnt[*Ts:
    AnyType](*xs: *Ts) -> Int` fails the same way. The pin prints `[1, 1]`
    for `n = 5`.
  - Checker: `ApplicableFunction` records the collector's index and its
    element `SlotType`, or the `*Ts` binder typing it. `Checker::applied_arguments`
    matches with `CallVariadics { positional: true, .. }`, applies each
    overflow argument at the element type, and binds a type pack through
    `resolve_use_params`, one `Param(Ts)` pattern per overflow argument.
  - Elaborator: `Specializer::demand_application` regroups the run at the
    declaration's `variadic_index` into one `CtValue::Tuple`.
  - VM: `VmBackend::thaw` gains a `VariadicList` target arm building what
    `mir::variadic_list::build_variadic_list` builds. The type-pack
    collector's `RuntimePack` arm exists.
  - `tests/checker_test.rs`'s "a callable over a pack is applied by no
    call yet" assertion flips.
  - Found while landing R1528 (2026-10-29).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1643 A method taking `self` applies at compile time only when
  builtin**

  `applicable_declarations` (`checker/comptime_validation.rs`) lists a
  method with `self` only when it is `@always_inline("builtin")` or
  returns `!kgen.deferred`, so an ordinary method called on a
  compile-time struct value denotes nothing in a signature, where the pin
  keeps it as the method's application with the struct as receiver.
  - Probe: `SIMD[DType.int8, L(n).ord()]` over a non-builtin `def
    ord(self) -> Int` of `@fieldwise_init struct L` is "not an associated
    comptime expression"; the pin names `L(Int(2)).ord()` in a mismatch
    and prints `[1, 1]` when matched.
  - Probe: `-> SIMD[DType.int8, Self.l.get()]` as the return type of a
    method of `struct Tagged[l: L]`, over a non-builtin `get`, fails the
    same way; the pin prints `1` for `Tagged[L(1)]`. In a body it already
    runs, lifted as a thunk.
  - Lever: list every `imm`/default-receiver method. The receiver is the
    struct value the elaborator thaws (`demand_application` already counts
    `has_receiver`).
  - Trap: R397's `len[T: Sized]` made every `len(x)` a candidate and the
    std package check took minutes. Time a cold package build
    (`MOJITO_CACHE_DIR=$(mktemp -d)`) before and after.
  - Found while landing R1528 (2026-10-29).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1644 A callee with a `ref` parameter, or with a callable or
  origin compile-time parameter, is no compile-time application**

  `ApplicableShape::applicable` (`checker/comptime_validation.rs`) admits
  only a `read` or `var` runtime parameter (`passed_by_value`) and drops a
  callee whose own compile-time parameters include a callable or an
  origin, where the pin applies each.
  - Probe: `SIMD[DType.int8, bc(String("ab"))]` over `def bc(ref s:
    String) -> Int` is "not an associated comptime expression"; the pin
    prints `[2, 2]`.
  - Library case: `StringSpan.__init__(ref [Self.origin] src: String)`,
    which also blocks a compile-time `StringSpan` built from a `String`.
  - The elaborator thaws the argument as a value; a `ref` slot needs it
    addressed in compile-time memory (`store_to_comptime_memory`), as the
    VM addresses a `ref` argument at run time.
  - Found while landing R1528 (2026-10-29).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1529 A tuple or list display in a parameter position is frozen
  element by element, not applied**

  `aggregate_value` (`checker/constraints.rs`) reads `(n, 1)` and `[n, 1]`
  as `CtValue` aggregates, where the pin keeps each as its literal
  constructor's application (`Tuple(n, Int(1))`, `List(n, Int(1),
  __list_literal__=NoneType(None))`), unfolded through the check even when
  closed.
  - A symbolic aggregate has no type (`ct_value_ty`), and contextual typing
    is tried only for a closed value (`Checker::converts_to_parameter`).
  - Probe: `g[(n, 1)]()` in `def f[n: Int]` over `def g[t: Tuple[Int,
    Int]]()` fails with "type mismatch for value parameter 't': expected
    Tuple[Int, Int], found (n, 1)". The pin prints `5 1` for `f[5]()`.
  - Probe: `g[(1, String("a"))]()` over `def g[p: Tuple[Int, String]]()`
    fails with "expected Tuple[Int, String], found (1,
    $comptime$main$c0())". The pin prints `1 a`.
  - Probe: `g[[n, 1]]()` over `def g[l: List[Int]]()` reading `comptime
    len(l)` and `comptime l[0]` fails with "expected List[Int], found [n,
    1]"; closed, `g[[5, 1]]()` fails the MIR verifier ("register type
    Array[Int, 2], declared List[Int]"). The pin prints `2 5` for both.
  - Probe: `var a: Tag[(n, 1)] = Tag[(n, 1)]()` and `Tag[(1 + n, 1)]`
    against `Tag[(n + 1, 1)]` are rejected; the pin runs both.
  - Probe: closed `Tag[(5, 1)]` against `Tag[(5, 2)]` must stay rejected, as
    the pin rejects it naming `Tag[Tuple(Int(5), Int(2))]`.
  - Probe: a closed construction inside a display is still frozen
    (`Checker::element_value`, `checker/constraints.rs`), as a display of
    applications has no parameter expression: `comptime r = [NC(1),
    NC(2)][0]` in `assets/ok/ctfe_projection_of_temporary_in_body.mojo`.
    Delete that freeze here.
  - Probe: `comptime r = [mk(1), mk(2)][0]` over `def mk(n: Int) -> NC`, `NC`
    only `Movable`, is lifted to a thunk whose return copies the element
    out of `__getitem__`'s reference result, so it is rejected with "cannot
    copy non-Copyable type 'NC'". The pin prints `1`.
  - The lever: `build_ct` builds a display as the `Apply` of the
    constructor `record_display_construction` selects for its contextual
    type, as a struct construction is applied
    (`Checker::called_application`). No new `ParamKind` node.
    `aggregate_value` goes.
  - Plan: `R1472-a-symbolic-tuple-list-or-struct-plan.md`, Slice 3.
  - Found while landing R1386 (2026-10-26).
  - Depends on R1528.
  - Model: Fable, Planned.
- [ ] **R1474 A predicate alias body or a struct header's condition is
  compiled without its binders in scope**

  `constraint_operand`, `predicate_operand`, and the `conforms_to` subject
  arm (`checker/constraints.rs`) read a bare name there as
  `ParamRef::unbound(name)`, bound by spelling later (`bind_constraint`,
  `apply_predicate_alias`), where upstream emits the alias body and the
  conformance condition as parameter expressions with their parameters in
  scope.
  - The one builder (`build_ct`) cannot serve these operands: it reports
    "not a compile-time Int constant: T" or `UnqualifiedStructParam`, so
    the name fallback stays beside it.
  - The lever: open the alias's or the header's binders before compiling
    the condition, so the builder reads them as it reads a body's.
  - Probe: `assets/ok/bool_alias_predicate_where.mojo` and
    `std/memory/maybe_uninit.mojo`'s header conditions (about 425 operands
    in the corpus) reach the fallback.
  - Probe: `struct P[*Ts: Movable] where Ts.length == 2` used as
    `P[Int, Bool].two()` stops with "unsupported generic constraint
    operand" (`constraint_operand`), and `where
    Ts.all_conforms_to[Copyable]()` in the same place with "unsupported
    generic where proposition" (`compile_generic_constraint`). The pin
    prints `2` for each, and a method's own `where Self.Ts.length == 2`
    runs.
  - Done when `constraint_operand`'s bare `Identifier` arm is gone.
  - Found while landing R1387 (2026-10-26).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1475 A `comptime if` condition is a `GenericConstraint`, not a
  `Bool` parameter expression**

  A type comparison is now one `ParamOp::Eq` as a value and a
  `GenericConstraint::Eq` as a condition, where upstream's
  `hlcf.comptime.if` takes its condition as one `i1` parameter value.
  - MIR's `comptime if` text, mono's `constraint_holds`
    (`mono/availability.rs`), and the checker's `compile_generic_constraint`
    all read the constraint form, so the two forms decide one comparison
    twice.
  - The lever, first session: the checker records the condition's `Bool`
    parameter expression (`build_ct`) on the `comptime if`, and mono
    decides from it where present; deleting the constraint form follows as
    its own entries.
  - Probe: `comptime if T == Box[Int]` needed the same type-identity fix in
    `fold_infix` and in `constraint_holds` (R1387).
  - Found while landing R1387 (2026-10-26).
  - Depends on R1474.
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
  - Probe: `var fns: Array[def(x: Int) capturing[_] -> Int, 1] =
    [lambda (x: Int) {var i} -> Int: x * i]`, an annotated display only
    R176's leniency accepts (the pin reports "no matching function in
    initialization"). An unannotated display holds the lambda's storage
    struct, which no erasure respells (R1378).
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

- [ ] **R1601 A VM struct value names the instance whose body built it, so
  one an erased (`_REMOVED_ARG`) clone returns finds no member of its
  caller's instance**

  `print(B[T, n].make(x))` in `def g[T: ..., n: Int]`, over `struct B[T:
  ImplicitlyCopyable & Writable & Deinitable, n: Int]` with a static
  `make(v: Self.T) -> Self`, stops the VM with "`B$mono$TString$V$REMOVED$ARG`
  has no `write_to` instance to display it", where the pin and the Pliron
  backend print `B(a)`.
  - `remove-unused-params` removes `n` from `make`, so its clone builds a
    `B[String, _REMOVED_ARG]`. The VM's display looks `write_to` up by
    that name (`struct_member_instances`, `vm/dispatch.rs`), and only the
    concrete `B[String, 3]` has one.
  - Upstream's struct value is its layout, which an unread parameter does
    not change. Lever: key a VM struct value's members by its template and
    layout, or retype the erased clone's result to its caller's instance.
  - `MOJITO_PRE_ELABORATION=0` runs it; `print(b.v)` on the result runs.
  - The same through `type_of(b).make(b.v)`.
  - Found while landing R1599 (2026-10-28).
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
  - Probe: `run_it(get, 1)` over `@always_inline("nodebug") def run_it(f:
    def(Int) capturing[_] -> Int, x: Int) -> Int: return f(x)` and a
    capture-list `def get(y: Int) {k} -> Int` keeps its call; once
    inlined, `canonicalize` folds the call through the closure to
    `get`'s method call on its storage
    (`tests/mir_passes/canonicalize/indirect_closure_storage.mir`).
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
    generic region path does through `region_generators`.
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

- [ ] **R1280 An iteration step through a bound keeps its abstract
  protocol after inlining**

  When `inline-parametric` substitutes `List[Int]` for a callee's `T:
  Iterable`, the inlined `for x in a` keeps its `call.method`s naming
  `__trait_dispatch.__iter__$ov$SelfRef` and
  `__trait_dispatch.__next__$ov$`, where upstream's substitution folds both
  witnesses.
  - The inliner's witness fold (`inline_parametric::fold_witnesses`) skips
    both calls: the iterator slot is typed by an associated
    projection (`T.IteratorType[o]`) only the elaborator solves.
  - Both calls already name the requirements the witness tables record.
  - Fold both at once, through `StructDeclaration::witness_tables`, once
    the substitution closes `T.IteratorType[o]` at a concrete struct; the
    `abstract_value_next` rules in `mir/verify/calls.rs` then meet only
    an unfolded step.
  - Probe: `fc(xs: List[Int])` returning `count(xs)`, for
    `@always_inline("nodebug") def count[T: Iterable](a: T) -> Int`
    counting `for _ in a`; done when `fc`'s inlined body names
    `List.__iter__` and the iterator's `__next__`.
  - Since R1288 lifted the inliner's `try` veto, `count` inlines into
    `fc`, so the probe shows the two unfolded calls.
  - Found while landing R1052 (2026-10-22).
  - Depends on R1288.
  - Model: Fable, Not Planned.

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

- [ ] **R1349 `automatic-inline` keeps a direct call of an undecorated
  capturing nested `def`**

  A plain nested `def` that captures, such as `def get() {imm y} -> Int`,
  stays a call in concrete MIR, where upstream's `AutomaticInline`
  inlines it below its threshold of 50 operations.
  - The elaborator clears a concrete declaration's `enclosing`
    (`mono/specializer.rs`), since an instance reads no enclosing binder.
  - `instantiated_callee` (`analysis/inline_parametric.rs`) accepts a
    call's `capture_accesses` only for a callee with an `enclosing`
    declaration, so the instance's direct call keeps them and is refused.
  - Lever: recognize a lifted body's direct call by its capture operands
    (the accesses are to places the call passes as leading arguments),
    not by the declaration's `enclosing`.
  - Probe: `outer(4)` over `var y = x` and an undecorated
    `def get() {imm y} -> Int: return y + 1` keeps
    `call "outer$get"` in concrete `main`; `def bump() {mut c}: c += 3`
    called twice keeps both calls.
  - A `nodebug` or `@always_inline` nested `def` inlines before
    elaboration, so only the undecorated form is affected.
  - Found while landing R1105 (2026-10-24).
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

- [ ] **R933 A span origin can name a slot no instruction addresses**

  `mir::addressed_slots`, the guard before `retire_slots`, ignores the
  slots that `function.spans` origins name, but `renumber_slots` renumbers
  them, so retiring a slot only a span names trips its debug assertion.
  - The unroller retires slots typed over a loop binder
    (`crates/mojito-native/src/native/mono/unroll.rs`), past its own "used
    outside its loop" check.
  - R837's probe hit it on `comptime_for_index_typed_local.mojo` when an
    erased marker kept its span. `lower-lit` now drops those spans.
  - Fix either side: the verifier refuses a span for a register no
    instruction defines, or `retire_slots` clears a retired slot's span
    origins, as `mem_2_reg` does.
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

- [ ] **R1575 Each infix operator over a `SIMD` checks and inlines its
  dunder chain per site, where upstream's parser folds a builtin call**

  Since R1553, `assets/ok/simd_compare_all_dtypes.mojo` takes 360 ms where
  it took 223 ms, and the corpus sweep's CPU time rose by about a fifth.
  - `check_program.bodies` grows from 15 to 34 ms there: each comparison
    resolves `__lt__` as an overloaded method call, its operand conversion
    included.
  - `inline-parametric` grows from 53 to 115 ms: each instance splices
    `__lt__`, then `lt`, then `__bool__`, all `@always_inline("builtin")`.
  - Upstream's `foldInlineBuiltinFunction` folds a builtin callee's body at
    the call once per instance.
  - Lever: memoize the dunder selection per operand types in the checker,
    and the spliced builtin body per instance in `inline-parametric`.
  - Measure with `--timings` on that fixture and on `b_cmp`-style loops;
    R1551's `+` and R1552's `&` pay the same cost.
  - Found while landing R1553 (2026-10-28).
  - Depends on nothing.
  - Model: Fable, Planned.

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

- [ ] **R541 Natively, an applied constant of a sized scalar type fails IR
  verification**

  Problem: `comptime W = lane(SIMD[DType.int32, 4](1, 2, 3, 4))` over
  `def lane(v: SIMD[DType.int32, 4]) -> Int32`, then `print(W)`, prints `4`
  on the VM and at the pin, and stops natively with "llvm.ptr is not
  builtin.integer or a vector of it".
  - An applied constant of type `Tuple` or `Int` runs natively; the frozen
    one-lane vector does not.
  - Found while landing R517 (2026-10-09).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R576 Natively, a module constant declared at a sized scalar type
  fails IR verification**

  Problem: `comptime S: UInt8 = 2`, then `print(S)`, prints `2` on the VM
  and at the pin, and stops natively with "llvm.ptr is not builtin.integer
  or a vector of it".
  - The bundled `std.os.os.SEEK_END: UInt8` fails the same way, with or
    without the compiled package.
  - An untyped integer constant (`O_CREAT`) runs natively.
  - R541 is the same failure for an applied constant of a sized scalar
    type, so one fix may close both.
  - Found while landing R563 (2026-10-10).
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
  - Pliron passes a one-field `TrivialRegisterPassable` struct as an
    aggregate by reference, where the pin passes its element by value:
    `define internal i64 @"u2::add(u2::L,u2::L)"(i64, i64)` for a builtin
    `__add__` on `L{v: Int}`, and `i1` for `B{v: Bool}`.
  - The fix: `lower/types.rs::lower_ty` lowers a struct
    `MirDeclarations::register_field` answers as its field, as the VM
    holds it. `struct.create` and `field.get` forward, a field place adds
    no GEP, and the layout is unchanged; only the signature classification
    moves, with the SIMD-by-value change above.
  - A later task that needs an ABI bump joins this entry rather than
    getting its own.
  - The signature classification changes the native calling convention itself,
    and the ABI version, `docs/native-abi.md`, the runtime, and the parity
    harness all move together.
  - Depends on nothing. Every later change that needs an ABI bump joins this
    entry instead of waiting on it.
  - Model: Fable, Planned.

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

- [ ] **R1570 A register-passable `read self` receiver keeps its place, so
  its owner outlives the call, where upstream passes it by value**

  `a.p.add(b.p)`, with `p` a `TrivialRegisterPassable` field of a `W` that
  prints in its destructor, prints `add` then `del 2`, `del 1` here, where
  the pin prints `del 1`, `del 2`, then `add`.
  - The method call retains the receiver's place (`recv_place`), so the
    owner's last use is the call, not the receiver's read at setup.
  - A read argument already decides this by the checker's
    `RegisterPassableRead` and passes such a value with no place unless
    the call's result borrows it (`lower_call_operand`).
  - `lower_receiver_value` (`mir/calls.rs`) drops the place for a `SIMD`
    receiver only: record the same fact for a receiver and apply it to
    every register-passable one.
  - Probe: the shape above, as an `assets/ok` drop-order fixture.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1603 A conditional's register-passable result that needs
  destroying is dropped before its use**

  `var p = Optional[Int](1) if c else Optional[Int](4)` fails with "invalid
  checked program: … var.store ends owned register, which some path
  reaching it has ended", where the pin prints the value.
  - Drop elaboration (`mojito-analysis` `drops.rs`) places a `drop.reg`
    of the `if`'s result register right after the `if`, ahead of the
    `var.store` that consumes it, so the store ends a dead register.
  - Only a result the checker proved register-passable is yielded as the
    `if`'s value (`RegisterPassableResult`); a memory-only one goes
    through a result slot and runs.
  - Probe: a user `RegisterPassable` struct with a printing destructor,
    `R(1) if c else R(2)`, fails the same way after `sroa`.
  - Probe: `o or Optional[Int](4)` over `o: Optional[Int]` (R97's
    `and`/`or` yield the same `if` result).
  - Found while landing R97 (2026-10-28).
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

- [ ] **R1095 A copy or move of a compile-time value parameter's slot
  runs no constructor**

  `var r = q` over a struct value parameter `q: Q` whose `__init__(copy)`
  prints runs no copy constructor, where the pin materializes `q` and
  calls it.
  - A read of a value parameter is its parameter constant (R662), which
    the elaborator materializes, a constructed value through its
    constructor, so `var r = q` copies the constant's register and no
    copy constructor runs.
  - Probe at 2026-10-26: a `Q(ImplicitlyCopyable)` whose
    `__init__(out self, *, copy: Self)` prints, read `var r = q`, prints
    no copy on the pin either. Recheck the claim before fixing.
  - Probe: `assets/ok/value_parameter_field_read.mojo` (`var r = q`),
    `assets/ok/def_aggregate_value_binder_served.mojo` (`p[0]` over a
    `Tuple[Int, Int]` binder).
  - Probe: `print(T.S)` over a `comptime S: String` member materializes
    the `String` as a register constant that a `deinit` call destroys,
    which R1445's rule rejects
    (`assets/ok/trait_comptime_requirement_default.mojo`, `show`).
  - Materialize the parameter's value into storage once per read, as
    upstream's `kgen.param.constant` is a value, and lower the copy or
    move of it to the constructor call as any other.
  - Found while landing R1046 (2026-10-16).
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
    initializers (R341), and a display element is not a forwarded
    argument.
  - Found while landing R730 (2026-10-13).
  - Depends on R341.
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

- [ ] **R1331 A loan-carrying temporary at a read parameter is destroyed at
  the statement's end, where the pin destroys it after the call**

  Problem: `print(look(Holder(Pointer(to=local))))` with `def look(h:
  Holder) -> Int` and a `Holder.__deinit__` that prints prints `7`, then
  `del 7`, on both backends, where the pin prints `del 7`, then `7`.
  - MIR anchors the temporary in a hidden `$arg_loan_r` slot to keep
    `local` borrowed through the call (`anchor_borrowing_argument`,
    `mir/calls.rs`).
  - The slot lives to the statement's `KeepAlive`
    (`flush_argument_anchors`, `mir/lower_stmt.rs`), so the temporary dies
    after `print`, where upstream destroys it once `look` returns.
  - Related: R123 (the same anchor at an owned parameter has two owners)
    and R691 (its loan conflicts until the statement's end).
  - Found while planning R1325 (2026-10-24).
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

- [ ] **R1501 A field moved out of an expression temporary leaves a hole no
  pass sees**

  `mk().s^` over `def mk() -> Q` reaches the call's temporary through a
  field projection, which upstream's `emitTransfer` admits; its
  `CheckLifetimes` then rejects the hole the temporary is destroyed with
  ("field '(expression temporary).s' destroyed out of the middle of a
  value").
  - Mojito admits the transfer too (`transfer_verdict`, `TransferRoot::
    Temporary`), but the temporary is no slot the move analysis
    (`analysis/moves.rs`) tracks, so the VM traps ("use after Pointer
    deallocation").
  - A named local reports the hole already (`var t = q.s^`).
  - The lever is materializing the temporary in a slot, as upstream's
    expression temporary is, so `FieldDestroyedOutOfTheMiddle` reaches it.
  - Probe: `var t = mk().s^` over `@fieldwise_init struct Q(Movable): var
    s: String`.
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
    initializers. A `def(idx: Int, var elt: T) capturing[_]` handler stands
    in for upstream's `Some[def(...)]` until R813.
  - Since dcbecf4e every list display calls the literal initializer, so
    this used to work through the compiler-built display.
  - Pinned by `conformance/probes/list_display_linear_elements.mojo`.
  - Depends on R1614 (the closure must package with the library).
  - Model: Opus, Not Planned.

- [ ] **R1614 The bundled library cannot hold a capture-list closure**

  A nested `def` with a capture list in a bundled library method fails the
  package build, so the library is compiled from source on every run.
  - The first refusal is the overload check
    (`InterfaceExport::records`): the closure's `__storage` struct and its
    body method are lowered declarations no source scan declares.
  - Past that, `DeclarationRecord::refuse_unjoinable` refuses a record
    holding a struct not its own: the storage struct is placed in its
    enclosing struct's record.
  - Found while porting upstream's `List` literal initializer (R1613).
  - Depends on nothing.
  - Model: Fable, Planned.

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
  - Probe: `print(take(Holder(Pointer(to=local))))` with `def take(var h:
    Holder) -> Int` and a `Holder.__deinit__` that prints prints `del 7`
    twice on both backends, where the pin prints it once.
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
  - Found while landing R743 (2026-10-23).
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

- [ ] **R701 A collection element passed to a `ref` parameter is rejected**

  Problem: `def c4(ref x: S) -> Int` called as `c4(l[0])` over `List[S]`,
  `S` only `Copyable`, fails with "value of type 'S' cannot be implicitly
  copied ... (ordinary value read through a reference result)", where the
  pin binds the element by reference.
  - The generic `ref x: T` form fails the same way.
  - Probe: a `read` parameter too: `value(pick(r))` over a `Movable`
    `Res`, `pick(ref r: Res) -> ref [r] Res`, is rejected, where the pin
    borrows the referent and prints its value.
  - Probe: a conversion's read operand too: `Bool(os[0])` and `Bool(t[0])`
    over a `List[Optional[T]]` and a tuple, `T` only `Copyable`, are
    rejected, while the condition `if os[0]:` runs. `expect_bool` marks
    such an element `borrowed_read_call_place`, but
    `borrow_nominal_place_argument` admits only a bare name.
  - Probe: a `mut` parameter too: `bump(ps[0])` over `List[P]`, `def
    bump(mut p: P)`, `P` only `Copyable`, is rejected the same way, where
    the pin writes the element in place (found landing R117, 2026-10-25).
  - Probe: a call's reference result read by a read consumer.
    `print(l.unsafe_get(0))` and `items.unsafe_get(0).write_repr_to(s)`
    over `List[T]` copy the element, and `show(l.unsafe_get(0))` is
    rejected even over a concrete `List[C]`, where the pin borrows both.
    `lower_borrowed_read_argument` returns `None` for a call, which spells
    no name (found landing R1354, 2026-10-25).
  - The rejection is the checker's implicit-copy test on a reference
    result (`crates/mojito-checker/src/checker.rs`), which treats the
    `ref` slot as a consuming read.
  - Found while landing R700 (2026-10-13).
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

- [ ] **R1573 A write through a parametric-mutability `ref` field's
  element is a loan conflict or a rejection**

  In `struct View[m: Bool, //, o: Origin[mut=m]]` with `var src: ref[o]
  List[C]`, a `mut self` method's `self.src[0].n = 3` fails ownership
  ("access to '$call_ref_r5.n' conflicts with live reference"), and
  `self.src[0].inc()` over a `mut self` `inc` is rejected ("expression
  must be mutable in assignment"), where `self.src[0] += 1` and
  `self.src[0] = x` run (`assets/extensions/ok/ref_field_parametric_mut_*`).
  - The hidden reference an accessor returns lends immutably when its
    mutability is a parameter (`materialize_call_reference_place`,
    `mir/calls.rs`), unless the caller writes through it in place, which
    only an augmented store tells it today.
  - Direct `ref` fields are a kept extension, so the fixture's rule
    ("judged per instantiation") is Mojito's: make the field store and the
    `mut` method call follow it as `+=` does.
  - Found while landing R1552 (2026-10-28).
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
  - Found while planning R1166 (2026-10-20).
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
  - Found while landing R1503 (2026-10-29).
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

- [ ] **R1584 A function type in a trait composition does not parse**

  `F: ImplicitlyCopyable & RegisterPassable & def[width:
  SIMDLength](SIMD[DType.int32, width]) -> SIMD[DType.int32, width], //`
  is a parse error ("Expected a trait name after '&'"), where the pin
  binds `F` to the function's type and runs `func[4](x)`.
  - The composition parser reads only trait names after `&`; upstream's
    reads a function type there as one more requirement of the binder.
  - `docs/grammar.md` first, then parser tests.
  - Upstream's closure overload of `SIMD.reduce` spells its binder this
    way (R1585).
  - Found while planning R1554 (2026-10-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1502 `Pointer` and `StringLiteral` are not `TrivialRegisterPassable`**

  Upstream declares both `TrivialRegisterPassable`, but the modelled
  conformance (`modelled_trivial_register_passable`, `mojito-types`) and
  the checker's `is_trivial_register_passable` answer no for
  `TyKind::Pointer` and `TyKind::StringLiteral`.
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

- [ ] **R592 A module function is visible only after its statement**

  Problem: a struct method calling a module `def` declared after the
  struct stops with "Undefined variable 'later'", where the pin prints `2`.
  - Probe: `struct S` whose `go` returns `later(self.x)`, with
    `def later(x: Int) -> Int` declared after it.
  - The check binds a module `def` at its statement in the source-order
    walk, and nothing predeclares one, as structs' and traits' signatures
    are.
  - A trait default reads every module function (R551), since the check
    checks it after the walk.
  - Probe: `stdlib/std/range.mojo`'s float `__next__` calls the
    `SIMD.fma` method where upstream imports `std.math.fma`. The import
    closes the cycle `std.range` → `std.math` → `std.sys` → … →
    `std.collections.list` → `std.range`, and either module's methods then
    precede the other's `def`.
  - Probe: `from std.math import max` in `builtin/builtin_slice.mojo` or
    `os/path/path.mojo` stops with "Undefined variable
    '__module$std$math$max'", since `std.math` links after them.
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
  - Probe: the bundled `Array`, `Optional`, `OptionalReg`, and `Span`
    omit upstream's `DevicePassable` conformance, a trait with
    `@staticmethod` requirements.
  - Found while landing R914 (2026-10-14).
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
  its message or its `use=` replacement), at a `@parameter` ("'@parameter'
  is deprecated; use '@__parameter'"), and at an `@export` without an
  `abi()` effect, where Mojito prints nothing.
  - The checker's warning channel already carries the match-coverage
    warnings the CLI prints.
  - `checker/decorators.rs` accepts all three decorators silently.
  - Probe: `conformance/probes/decorator_warnings.mojo`.
  - Found while landing R192 (2026-10-19).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1165 A trait method's caller-contract decorators are rejected**

  Problem: `@__unsafe_nested_origins_read_only`, `@__parameter` or
  `@parameter`, and `@implicit` on an `__init__` requirement are accepted
  on a trait method at the pin, but Mojito reports each as unsupported.
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

- [ ] **R1448 An implicit conversion never selects a constructor with
  parameters of its own**

  Problem: an `@implicit` constructor that declares its own parameters,
  such as `Variant`'s `__init__[T: Movable](out self, var value: T)`, is
  never chosen for an implicit conversion, so `v = String("hi")` over a
  `Variant[Int, String]` is rejected with "type mismatch for assignment to
  'v'", where the pin converts and prints `hi`.
  - Cause: `implicit_conversion_constructor` (`mojito-checker/checker.rs`)
    keeps a constructor only when the source `coerces` to its parameter
    type, after substituting the struct's binders alone. A parameter typed
    by the constructor's own binder (`T`) never matches, so the
    constructor's binders are never inferred from the source.
  - The selected conversion records the struct's binders
    (`conversion_instantiation`) and nothing for the constructor's own, so
    lowering needs those too.
  - Probe: `var v: Variant[Int, String] = String("hi")` and `v =
    String("hi")` print `hi` at the pin.
  - Probe: a struct `W` with `@implicit def __init__[T: Writable](out self,
    s: T)` beside `def __init__(out self, x: Int)`: `w = String("hey")`
    runs the generic constructor at the pin and is rejected here.
  - Found while landing R1405 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R1541 A compound value-parameter pattern solves no binder**

  Problem: `g[w: Int](x: B[w * 2])` called as `g(B[n * 2]())` in `def
  f[n: Int]()` is rejected with "cannot infer type parameter 'w' of 'g'
  from the arguments", where the pin unifies the two expressions node by
  node, binds `w` to `n`, and prints `2`.
  - Cause: `solve_value_args` and `solve_applied_args`
    (`checker/generics.rs`) bind only a bare binder or the arguments of two
    applications of one function. Any other pattern (`w * 2`, `w + 1`) is
    left for the substituted signature to compare, so its binder stays
    unsolved.
  - Lever: unify a canonical `ParamKind::Op` pattern against an actual of
    the same operator and arity operand by operand, binding a binder
    operand and requiring every other operand equal.
  - Probe: `B[w + 1]` against `B[n + 1]` solves `w = n` at the pin.
  - Probe: `show[w: Int](x: SIMD[DType.int32, w * 2])` against
    `SIMD[DType.int32, n * 2]` solves `w = n` and prints `2 [1, 1, 1, 1]`
    at the pin. Its slot holds `SIMDLength(w * 2)`, so the rule applies to
    the construction's argument.
  - Probe: the pin rejects `B[w * 2]` against a closed `B[4]`: it unifies
    structure and never inverts an operation.
  - Found while landing R382 (2026-10-27).
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

- [ ] **R1466 A call on a struct value is checked through its `def(...)`
  conformance, not as its `__call__` method call**

  `a(x)` over a struct value is checked against the contract
  `declared_callable_contract` (`checker/call_inference.rs`) finds and
  lowered as a `call.indirect` the elaborator devirtualizes, where
  upstream resolves it as the method call `a.__call__(x)`.
  - Probe: `print(a(1))` over `var a = Add(3)`, with `struct
    Add(Copyable)` declaring `def __call__(self, x: Int) -> Int`, prints
    `4` at the pin; Mojito reports "'a' has type Add and is not callable".
  - Probe: `return f(x)` in `def apply[F: Caller](f: F, x: Int) -> Int`,
    with `trait Caller` declaring `__call__`, prints `7` at the pin over
    `apply(Add(3), 4)`; Mojito reports "'f' has type F and is not
    callable".
  - Lever: check a call on a non-function value as R1357 checks an
    operator, as its `__call__` method call, and lower it as a
    `call.method`; the specializer's `CallIndirect`-over-struct
    devirtualization (`native/mono/specializer.rs`) then goes.
  - That devirtualization is the last place `native::mono` composes a
    callee's name (`symbol::retarget_method_symbol` on the contract's
    `__trait_dispatch.__call__…`), as is its `call.indirect` check in
    `mir/verify/calls.rs`; both go with it.
  - The devirtualization names a reduced `__call__` without dropping its
    operands ("'Add$mono$V1.__call__$REMOVED$ARG' expects 0 argument(s),
    got 1" once `__call__` leaves `mir::TYPE_REACHED_MEMBERS`); a
    `call.method` reduces through `Specializer::reduce_method_call`.
  - Upstream's `SubscriptNode` also binds a bracket over such a value to
    `__call__`'s parameters when the struct declares no `__getitem__`,
    `__setitem__`, or `__getattr__`; Mojito has no such path.
  - Found while landing R1031 (2026-10-26).
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

- [ ] **R59 A static method read through its struct as a value is
  rejected**

  Problem: `var f = S.st` over `@staticmethod def st(x: Int) -> Int` fails
  with "Undefined variable 'S'", where the pin prints `4` for `f(3)`.
  - The checker reads the bare struct name `S` as a value before any
    static-method path sees the read.
  - The explicit-argument call `P.plain[Int](4)` this entry first named
    already runs (checked 2026-10-29).
  - The plan must first find which pass infers the bare type name as a value.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R60 A generic `comptime` alias is not constructible**

  Problem: `W[Int](8)` over `comptime W[T: AnyType] = Pt[T]` fails with
  "Undefined variable 'W'", where the pin constructs `Pt[Int]`.
  - A closed alias's call already constructs the type it names
    (`infer_type_alias_read`, `checker/initializer_list.rs`), and so does
    a call through a struct's type member (`Holder[7].Same()`). A
    generic alias's application has no such read.
  - Probe: over a pack, `T2[Int, Bool](1, True)` for `comptime T2[*Ts:
    Copyable] = Tuple[*Ts]`, and `comptime for v in V[1, 2, 3]()` for
    `comptime V[*vs: Int] = ParameterList[vs.values]`, fail with
    "Undefined variable", where the pin prints `True` and `1 2 3`.
  - Probe: a local `comptime a = A[4]()` over `comptime A[k: Int] = W[k]`
    fails with "Undefined variable 'A'", where the pin constructs `W[4]`.
  - Probe: a partially bound alias, `Bytes[4](1, 2, 3, 4)` over `comptime
    Bytes = SIMD[.uint8, _]` and `Self._T[width](v)` over a member `comptime
    _T = SIMD[Self.dtype, _]`, fails with "Undefined variable 'Bytes'" and
    "Undefined variable 'Self'", where the pin constructs the vectors.
    Upstream's `_b64encode.mojo` (`OFFSETS`) and `SIMD.reduce` construct
    this way.
  - Depends on R59, which finds the pass that reads a bare type name as a
    value; the alias call needs the same answer.
  - Model: Opus, Planned.

- [ ] **R1598 The `assert` statement does not parse**

  `assert n == 3, "boom"` is a parse error ("Expected newline, ';', or EOF
  at the end of statement"), where the pin desugars it to `debug_assert()`
  and, under the default `-D ASSERT` mode, skips it, so the next
  `print("ok")` runs.
  - Bundle the part of `std/builtin/debug_assert.mojo` the desugaring
    calls, with `ASSERT_MODE` from `get_defined_string`.
  - Upstream's `SIMD` element-list initializer (R1592) asserts its arity
    this way.
  - Grammar first.
  - Found while landing R1555 (2026-10-28).
  - Depends on nothing.
  - Model: Fable, Not Planned.

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

- [ ] **R1544 `SIMD[dt, _](v)` does not bind its width from a vector
  argument**

  `SIMD[DType.int32, _](v)` and `SIMD[DType.float64, _](w)` over a
  four-lane `float64` `v` and a two-lane `float64` `w` print `[1, 2, 3, 4]`
  and copy `w` at the pin. Mojito rejects both: "type mismatch for SIMD
  element: expected a DType.int32 element, found SIMD[DType.float64, 4]".
  - The declared cast overload `__init__[other_dtype](value:
    SIMD[other_dtype, Self.length])` does not bind an unbound `_` from its
    argument, so the construction falls back to the checker's rule, whose
    `argument_simd_width` admits only a one-lane argument.
  - A user struct of the same shape, `W[DType.int32, _](v)` over
    `__init__[other: DType, //](out self, x: SIMD[other, Self.n], /)`, is
    rejected at the pin too ("depends on an unresolved parameter 'n'"), so
    find what lets `SIMD`'s `_` bind before changing the general rule.
  - Found while landing R186 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1545 A `SIMD[dtype](...)` call with its length omitted is
  rejected, where the pin binds the length from the arguments**

  `print(SIMD[DType.int8](5))` prints `5` at the pin, which leaves
  `length` unbound as `SIMD[DType.int8, _](5)` does. Mojito stops with
  "type 'SIMD' expects 2 type argument(s), got 1".
  - Cause: `Checker::simd_dims` (`checker/type_resolution.rs`) requires
    both bracket arguments, and the construction dimensions `infer`
    records read it too.
  - Lever: a construction with one bracket argument takes the unbound
    width `argument_simd_width` already binds for `_`.
  - Found while landing R186 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1602 An `and`/`or` left operand that converts to the right
  operand's type is rejected, where upstream tests it before converting it**

  `n or W(raw=50)`, with `n = 2` an `Int` and `W` taking an `@implicit`
  `Int` constructor, prints `W20` at the pin; Mojito rejects it ("a left
  operand of 'or' converted to the right operand's type").
  - Upstream's `emitAndOr` tests the left operand's own `Bool`, then
    converts it to the common type in its arm; Mojito's
    `Flatten::expr_with_adjustments` applies a recorded conversion where
    the operand is evaluated, so the test would read the converted value
    (`W.__bool__`, giving `W50`).
  - Fix: evaluate the left source unconverted (`expr_unconverted`), test
    it, and apply its recorded conversion in its arm
    (`Flatten::short_circuit`); drop the checker's rejection in
    `Checker::infer_short_circuit`.
  - Probe: `Int32(0) or v` and `Int32(2) or v` over a 4-lane `int32`
    vector print `[1, 0, 3, 4] [2, 2, 2, 2]` at the pin, splatting the
    tested scalar; Mojito rejects both the same way.
  - Found while landing R97 (2026-10-28).
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

- [ ] **R393 A `def` cannot call a `def` declared below it**

  Problem: `def a() -> Int: return b(2)` above `def b(n: Int) -> Int` stops
  with "Undefined variable 'b'", where the pin runs it.
  - Module functions are declared in source order, each as its body is
    checked (`check_block`, `checker/statements.rs`).
  - A signature or a field type that applies a function declared below it
    already runs, since the functions a compile-time call applies are
    collected before any declaration (`applicable_functions`,
    `checker/comptime_validation.rs`).
  - Found while landing a function applied in a signature (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

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

- [ ] **R399 A method given a caller's parameter as its own value
  parameter leaves an untyped register**

  Problem: `return g.plain[b]()` in `def user[a: Int, b: Int](g: G[a]) ->
  Int`, for `def plain[m: Int](self) -> Int` in `struct G[k: Int]`, fails
  with "invalid checked program: fn 'user': register r1 has no checked
  type", where the pin runs it.
  - A literal argument (`g.plain[4]()`) in the same `def` runs.
  - The same call on a concrete receiver in `main` (`g.plain[3]()`) runs.
  - The bracket argument is lowered as a read of the binder `b`, and that
    read has no checked type on this path.
  - Found while landing the callee shapes a signature applies (2026-10-06).
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

### Catch Up To Current Mojo: Compile-Time Parameters, Packs, And Reflection

Track: `comptime`.

Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

- [ ] **R1571 A compile-time operator over a `comptime` constant
  annotated with a sized integer does not wrap to its width**

  `comptime Q: UInt8 = 5; comptime H = Q + 254` prints `259`, where the
  pin prints `3`: the constant folds as an unbounded integer, not a
  `UInt8`.
  - Probe: `-Q` prints `-5` (pin `251`) and `Q << 6` prints `320` (pin
    `64`); a local `comptime k: UInt8 = 5` does the same.
  - `comptime J = UInt8(5) * 100` folds right (`244`), so the
    annotation's conversion, not the fold, loses the width.
  - R602 (a local annotated sized constant is no parameter value) and
    R576 (natively, such a module constant fails IR verification) are the
    same annotation's other symptoms; one fix may close all three.
  - Found while planning R1552 (2026-10-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1604 A symbolic compile-time `and`/`or` takes only integer or
  `Bool` operands of one type**

  `def h[F: Float64, G: Float64 = F or 1.5]()` and `def k[N: Int, B: Bool
  = N or False]()` print `1.5 2.0 False True` for `h[0.0]()`, `h[2.0]()`,
  `k[0]()`, and `k[3]()` at the pin; Mojito rejects both ("over a
  symbolic parameter value of another type").
  - Upstream folds the operator to `POC::Cond` on the left operand's
    `Bool`, after converting both operands to a common type or each to
    `Bool`.
  - `Checker::short_circuit_value` builds the test as `x != 0` over an
    integer, so a float's test, and the `Bool` of each operand when the
    types differ, have no symbolic form yet.
  - Fix: build the symbolic test as the `Bool(x)` application the
    elaborator folds per instance, as a closed operand's `ct_truth` does.
  - Found while landing R97 (2026-10-28).
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
  - Found at the 2026-09-21 re-pin and left unfiled; refiled at the
    2026-10-17 re-pin.
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

- [ ] **R1538 An annotated `comptime` binding does not convert its value
  through the annotation's `@implicit` constructor**

  `comptime w: L = 4`, where `L` declares `@implicit __init__(out self,
  value: IntLiteral, /)`, keeps the literal `4` as the binding's
  compile-time value, where the pin binds `L(4)`.
  - Probe: `comptime w: L = 4; f[w]()` in `main`, over `def f[w: L]()`,
    fails with "not a compile-time Int constant: w". The pin prints `4`.
  - Probe: module `comptime W: L = 4` and `f[W]()` fail with "type
    mismatch for value parameter 'w': expected L, found 4". The pin
    prints `4`.
  - Probe: `comptime t: StaticString = "lit"` then `name[t]()`, over
    `def name[s: StaticString]() -> String`, local and module, fail the
    same two ways. The pin prints `lit`.
  - Probe: `comptime n: Int = 3` in `main`, then `var x: UInt8 = n` or
    `SIMD[DType.uint8, 2](n, n)`, run, where the pin rejects "cannot
    implicitly convert 'Int' value to 'UInt8'" and "no matching function
    in initialization": the binding stays an `IntLiteral`, which converts
    to any lane. A module `comptime N: Int = 3` does the same.
  - Probe: `comptime g = x // 0` over a module `comptime x: Int = 7`, then
    `print(g)`, fails with "keeps the parameter constant `7 // 0`", where
    the pin prints `0`: the literal pair's `//` by zero does not fold (R1624),
    where an `Int`'s folds to `0`.
  - Cause, module: `Checker::fold_module_constants`
    (`checker/constraints.rs`) folds the initializer to an `IntLiteral`
    before any struct registers, ignoring the annotation, and the later
    passes declare the constant `IntLiteral`.
  - Cause, local: the annotated arm of the `Comptime` statement
    (`checker/statements.rs`) records the folded literal in `comptimes`
    and binds no compile-time value of the annotation's type.
  - Lever: once the annotation resolves to a struct the value is not,
    bind `Checker::implicit_parameter_application` of the value, as a
    parameter argument (R1533) and an application's argument already
    convert.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1645 A value-pack element an `@implicit` constructor converts
  has no compile-time value**

  `f[1, 2]()` over `def f[*xs: W]()`, where `W` declares `@implicit
  __init__(out self, v: IntLiteral)`, fails elaboration with "value
  parameter `*xs` is not compile-time constant", where the pin prints `2`.
  - The checker's pack loop (`checker/declarations.rs`, "Each argument
    binds one value of the pack's element") records each element's
    converted application, but the application does not reach the call's
    `MirParamArg::expr` (`MirLowerer::param_arg_regs_folding`), so
    `mono::unify::bind_explicit_value_arguments` finds no constant.
  - `f[W(1), W(2)]()` runs, and so does a pack of one forwarded binder
    (`_get_kgen_string["llvm.", name]()`).
  - Probe: `cnt["a", "b"]()` over `def cnt[*extra: StaticString]() -> Int:
    return len(extra)`, which prints `2` at the pin.
  - Probe: `popcount["ct", "pop"](Int64(7))` over `def popcount[*parts:
    StaticString](x: Int64)` calling `pop.call_llvm_intrinsic` with
    `intrin=_get_kgen_string["llvm.", *parts]()`, which prints `3` at the
    pin.
  - Found while landing R1641 (2026-10-29).
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
  - Found while landing R1641 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1606 Automatic parameterization covers only a `SIMD` argument**

  `def f(x: List) -> Int` and `def h(x: List[_]) -> Int` reject their
  calls, where the pin parameterizes each over the unbound parameters and
  `print(f([1, 2]), h([1, 2, 3]))` prints `2 3`.
  - Upstream's automatic parameterization turns each unbound parameter of
    an argument's type (bare, `_`, or `...`) into an infer-only binder of
    the callee.
  - Mojito desugars only `SIMD` (`desugar_simd_wildcard_parameters`,
    `crates/mojito-comptime/src/comptime/synth.rs`), whose two binders it
    names by hand.
  - Probe: the bare `List` reports "expected List[_], found List[Int]";
    `List[_]` reports "type '_' for parameter 'T' does not conform to
    trait 'AnyType'".
  - The lever: build the binders from the struct's declared parameters,
    so the `SIMD` case becomes one instance of the general rule.
  - Probe: a value parameter annotated with a bare struct whose parameters
    are infer-only is not parameterized either. With `struct T[n: Int, //]`,
    `def g[t: T]()` called as `g[T[n=4]()]()` reports "expected T, found
    T[4]", and `def g[t: T = T[n=4]()]()` reports "not a compile-time Int
    constant: default for parameter 't'", where the pin runs both. Upstream's
    `size_of[…, target: CompilationTarget = CompilationTarget.current()]`
    is this shape (R1634).
  - Probe: a bare partially bound alias, `def f(x: I32) -> Int` over
    `comptime I32 = SIMD[DType.int32, _]`, reports "cannot infer type
    parameter 'length' of 'I32' from the arguments", where the pin prints
    `4` for a four-lane argument.
  - Found while landing R1568 (2026-10-28).
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
    members (measured 0 corpus diffs while planning R1370); R600's
    resolved-signature mangling is the full fix.
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
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1572 A method call on a module `comptime` constant of a builtin
  scalar type is rejected**

  `comptime X: Int = 12`, then `print(X.__str__())` or `print(X.__neg__())`,
  fails with "type 'Int' has no method", where the pin runs both.
  - The same call on a parameter (`n.__neg__()` in `def f[n: Int]()`), on a
    construction (`Int(12).__neg__()`), or on a run-time `Int` runs, so the
    receiver's resolution as a module constant, not the member, misses
    `SIMD`'s declaration.
  - Probe: `comptime W = X.__neg__()` and `comptime Y = X.__and__(5)` print
    `-12` and `4` at the pin, at module scope and in a body.
  - Found while landing R1552 (2026-10-28), which retired R684's run-time
    shape.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R602 A local `comptime` constant annotated with a sized scalar is
  no parameter value**

  `comptime c: Int8 = 5` (or `= Int8(5)`) then `show[c]()`, for
  `def show[x: Int8]()`, fails with "not a compile-time Int constant: c",
  where the pin prints `5`.
  - `Checker::bind_template_comptime` (`checker/comptime_validation.rs`)
    folds a closed initializer to its constant only when the binding is
    unannotated ("which an annotation would convert"): no parameter
    expression converts the constant to the annotated type.
  - `comptime c = Int8(5)` runs, and so do annotations of `Int`, `UInt`,
    and a struct (`comptime e: E = E(7)`).
  - Probe: `comptime c: Float32 = 1.5` then `show[c]()` for
    `def show[x: Float32]()`, in a body with or without binders (pin: `1.5`).
  - The original shape, `comptime y: Int = 5` then `f[y]()`, now runs.
  - Found while landing R601 (2026-10-10); rescoped while landing R1372
    (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R532 A generic closure declared in a generic body does not
  elaborate when bound to a callable parameter**

  Problem: in `def outer[m: Int]()`, a `@parameter def add[n: Int](value:
  Int)` reading `m`, bound by `invoke[add]()`, fails with "operator Add is
  not defined for Int and None"; the pin prints 53.
  - The folded callable names the lifted template alone, so the callee's
    instance specializes it without the enclosing instance's binders.
  - A nested body inherits them only where it is called from its defining
    frame (`declaration.enclosing == Some(owner)` in the `CallIndirect` arm
    of `mono/specializer.rs`).
  - Fails whether the closure captures or not.
  - Pinned by `conformance/probes/generic_closure_in_generic_body_callable_parameter.mojo`.
  - Found while landing R526 (2026-10-09).
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

- [ ] **R481 A value binder typed by a lane type is rejected**

  Problem: `s[dt: DType, v: Scalar[dt]]() -> Scalar[dt]` called as
  `s[DType.int16, 5]()` is rejected with "type mismatch for value parameter
  'v': expected Scalar[dt], found 5", where the pin prints `6`.
  - The argument is checked against the declared type before the earlier
    binder `dt` is substituted into it.
  - Probe: a struct application, `D[DType.int8, 5]` over `struct D[dt:
    DType, v: Scalar[dt]]`, fails the same way, and so does `Y[5]` over
    `comptime Y = D[DType.int8, _]`; the pin prints `5` for both.
  - Found while landing R477 (2026-10-07).
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

- [ ] **R893 A conditional expression over a sized-integer value parameter
  is a run-time `if`, where the pin selects its arm per instance**

  `1 if x == 0 else 2 * f[x - 1]()` in `def f[x: UInt8]()` prints `8` on
  the pin for `f[3]()`: a sized integer's comparison is a parameter value.
  - `Checker::parameter_valued_condition`
    (`checker/comptime_validation.rs`) admits only `Int`, `UInt`, `Bool`,
    and literal operands, so this conditional elaborates both arms and
    recurses past the instance budget.
  - Probe: the `UInt8` shape prints `8` only because `x - 1` wraps, so
    the 256 instances fit the budget; over `UInt16` it stops with
    "unsupported instantiation past the 4096-instance budget" (pin: `8`).
  - A `Float64` operand stays run-time: the pin runs out of memory on the
    float twin of the probe.
  - Found while landing R887 (2026-10-14).
  - Depends on R482.
  - Model: Opus, Not Planned.

- [ ] **R483 A method's explicit value parameter supplied from the caller's
  own binder fails MIR typing**

  Problem: `s.m[n](a)` or `S.st[n](a)` inside `def by_value[n: Int](a:
  Float64)`, against `def m[n: Int](self, a: Float64) -> Int`, fails with
  "register r4 has no checked type (UseVar …)", where the pin prints the
  method's result.
  - The same argument as a literal (`s.m[5](a)`) runs, and so does a free
    `def`'s `k[n](a)` from the same body.
  - MIR emits the binder as a runtime register, but the checker records no
    type for the argument expression on the method path.
  - Probe: a local `comptime u = pick[d]()` passed the same way,
    `v.cast[u]()` in `def f[d: DType, w: SIMDLength](v: SIMD[d, w])`,
    fails with "register r1 has no checked type (UseVar …)"; the pin runs
    it.
  - Found while landing R478 (2026-10-07).
  - Depends on nothing.
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

- [ ] **R501 A struct method cannot iterate a module `comptime` list a call
  builds**

  Problem: `comptime ML = mk(3)` at module scope, then `comptime for x in
  ML:` in a struct's method, stops with "function instantiation in
  parameter domain that recursively requires itself: the initializer of
  'ML' reads 'ML'", where the pin prints `0`, `1`, `2`.
  - The same loop in a `def` runs, and so does a literal module list, so
    the cycle the message names is a false one.
  - Probe: `conformance/probes/module_call_list_in_method_loop.mojo`.
  - Found while landing R310 (2026-10-08).
  - Depends on nothing.
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
  - The lever once R1529 lands: bind each local display binding as its
    literal constructor's application, which identifies by structure.
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

- [ ] **R901 An `Array` value parameter is rejected**

  Problem: `def show[xs: Array[Int, 3]]()` called as `show[L]()` over
  `comptime L = [1, 2, 3]`, or as `show[[4, 5, 6]]()`, fails with "type
  mismatch for value parameter 'xs': expected Array[Int, 3], found a
  non-materializable compile-time value", where the pin prints the
  elements; `struct Box[xs: Array[Int, 2]]` as `Box[[1, 2]]` fails the
  same way.
  - `Checker::ct_value_ty` (`checker/traits.rs`) answers `None` for a
    display application, and `List[T]` rather than `Array[T, n]` for a
    `CtValue::List`.
  - The bound value is the frozen `Array` a display's CTFE builds, which
    `native::mono` already materializes as one `Const::Value` per read.
  - Shrunk by R1529, which binds a closed list display as its
    constructor's application rather than a frozen `CtValue::List`.
  - Found while landing R789 (2026-10-14).
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
  - Probe: a type pack's member spelled out, `comptime E =
    Ts.__getitem_param__[1]` ("Undefined variable 'Ts'"), and `comptime G
    = TypeList[Ts.values].__getitem_param__[1]`, `comptime if
    Ts.__getitem_param__[0] == Int:`, and a struct member `comptime First =
    Self.Ts.__getitem_param__[0]`.
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

- [ ] **R1637 A parameterized `comptime` member whose body is a value is
  not read as a type in a type annotation**

  `def take[*Ts: AnyType](x: TypeList[Ts.values]._get_type_at_index[...])`
  is rejected ("'TypeList[AnyType, Ts]._get_type_at_index' is a
  compile-time value member: expected a type, found a value"), where the
  pin reads the member's type-valued attribute as the parameter's type
  and `take[Int](5)` prints `took`.
  - `Checker::resolve_parameterized_member` (`checker/type_resolution.rs`)
    takes only a `MemberTemplate::Type`, a body the checker spells as a
    type (`assoc_body_source_type`); a bracketed `__mlir_attr` body is a
    value template even when its value is a type.
  - The same member as a type argument (`List[…_get_type_at_index[…]]`,
    `name[…]()`) or in a `comptime assert` already reads through
    `build_ct`.
  - Probe: `var v: L[Ts.values].get[1]` over a user struct's `comptime
    get[idx: SIMDLength] = __mlir_attr[`#kgen.param_list.get<:`, …]`
    fails with "type mismatch for associated type parameter 'idx':
    expected a type, found a value".
  - Lever: an annotation over a value template builds the application
    through `build_ct` and takes its type when its meta is `Type`.
  - Found while landing R1536 (2026-10-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1148 A struct parameter read through a value is rejected**

  Problem: `pl.values` of a `comptime pl = ParameterList[vs.values]()` and
  `s.n` of a `var s = S[3]()` fail, where the pin answers each with the
  instance's argument (`comptime pl2 = ParameterList[pl.values]` then
  `pl2.size` prints `2`, `print(s.n)` prints `3`).
  - A read through a type works since R807 (`S[7].n`, `vs.values`):
    `Checker::struct_parameter_at` (`checker/constraints.rs`) answers it.
  - The value-receiver paths never ask it: `struct_value_field` (a
    compile-time struct value) and the instance arm of `infer_member`
    (`checker/indexing.rs`, a runtime value).
  - Probe: `_ParameterListIter[*pl](0)` over that `comptime pl` (the pin
    runs it), since `Checker::parameter_list_spread` takes only a literal
    `ParameterList[...]()` construction, not a value whose type is one.
  - Probe: a construction's `values` in a `comptime` body, `comptime
    _cases = TypeList.of[Trait=AnyType, NoneType, Int]().values` in a
    struct (also over `Self.T`), read as `TypeList[O._cases].length`, fails
    with "unsupported associated comptime member access", and at module
    scope with "Undefined variable 'TypeList'", where the pin prints `2`
    for each.
  - Found while landing R807 (2026-10-19).
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

- [ ] **R1190 A `!kgen.string` value has no runtime form**

  Problem: `var x = "a".value` is rejected ("a '__mlir_type.`!kgen.string`'
  value read at run time is not supported"), where the pin materializes the
  literal's KGEN string as a runtime value.
  - `TyKind::KgenString` has no layout, no VM value, and no native
    representation. The checker rejects a runtime read of `.value` or of a
    `!kgen.string` binder (`comptime_validation::kgen_string_runtime_read`),
    and translation legality rejects a register, slot, or signature that
    holds one once the pipeline has run (`llvm_translation_findings`,
    `verify/concrete.rs`). The VM holds one as a `Value::Attribute` only
    inside a compile-time evaluation (`_get_kgen_string`).
  - Probe: a runtime parameter `def f(x: __mlir_type.`!kgen.string`)`
    called as `f("a".value)`, which the pin runs.
  - Probe: `var l = List[KGENString]()` then `l.append("a".value)` and
    `print(len(l))`, which prints `1` at the pin. Mojito rejects it ("not
    a compile-time Int constant: KGENString").
  - Probe: `ParameterList[names]()[0]` over `*names: KGENString`, which
    reads the elements' runtime storage through `get_span`.
  - Probe: `G[name()].x()` over `def name() -> __mlir_type.`!kgen.string``
    returning `"abc".value`, which prints `2` at the pin. Admitting
    `KgenString` where R1630 admits `!kgen.deferred` (the
    `deferred_result_bodies` carve-out and the two lifting filters in
    `comptime_validation.rs`) is not enough: the thunk of `comptime n =
    name()` reads `"abc".value` as a field of a run-time `Str` ("field
    access on non-struct String"), so the literal's KGEN string needs this
    entry's run-time form.
  - A sibling of R1149's raw-list runtime form.
  - Found while landing R1183 (2026-10-20).
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
    `ran` at the pin (R1600 names it too).
  - Probe: `print(take(__mlir_attr.`#pop.fmf<none>`))` over `def take(a:
    __mlir_type.`!kgen.deferred`) -> Int: return 7`, with and without
    `@no_inline`, which prints `7` at the pin. The prototype left a dead
    `const` of the attribute (and its drop) in `main`, which `canonicalize`
    does not erase and the pin's DCE does.
  - Probe: `var a = pick(n > 5)` where `pick` also prints, then
    `print(take(a))`: the pin rejects it with "failed to convert func
    signature", since the printing callee survives.
  - `assets/type_error/mlir_deferred_runtime_call.mojo` stays rejected, then
    by translation legality ("failed to convert call result type").
  - Update `docs/features.md`'s `__mlir_op` row, whose run-time-call and
    "run-time attribute local" clauses this entry retires.
  - Found while planning R1630 (2026-10-29).
  - Depends on nothing.
  - Model: Fable, Planned.

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

- [ ] **R1485 An unbound application `Alias[a, _]` or `Alias[b=1, ...]`
  is rejected**

  Problem: `apply[Add[10, _]]()`, `apply[Add[b=1, ...]]()`, and `comptime
  P = Add[100, _]` then `P[1]` print `13`, `4`, and `101` at the pin, where
  Mojito rejects each with "type mismatch for value parameter 'b':
  expected Int, found a non-materializable compile-time value".
  - `Checker::apply_parametric_value` binds the bracket list through
    `resolve_use_params`, which has no unbound slot, so `_` and `...`
    resolve as values. Upstream marks a `_` argument explicitly unbound
    (`ParamInf.cpp`), installing no default for it, and a trailing `...`
    unbinds every slot left, defaulted ones included.
  - `ParamContext::bind_params` already folds a partial binding of a
    constant generator to the generator over the open slots; only the
    bracket list's binding is missing.
  - Probe: `Add[_, _]`, `Add[...]`, and `Add[a=_, b=5]` applied print `3
    7 6`.
  - Probe: over `Add3[a, b, c]`, `Add3[1, _, 3]`, `Add3[_, 2, ...]`,
    `Add3[1, ...]`, and `Add3[b=2, ...]` print `7 11 7 3`.
  - Probe: over `AddD[a: Int, b: Int = 10]`, `AddD[...]` then `P[1, 2]`
    and `AddD[1, ...]` then `Q[5]` print `3 6`, and `AddD[_]` then `Q[5]`
    prints `15`.
  - Probe: `print(Add[1, _])` is rejected at the pin as not `Writable`,
    its type `__generator_type[b: Int] Int`.
  - A partially bound struct (`Box[Int, _]`) is no generator at the pin
    and keeps today's meaning.
  - Plan: `R808-a-generator-type-t-parameter-and-plan.md`, §S7.
  - Found while landing R808 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Planned.

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

- [ ] **R1486 A type-valued generator is neither passed unapplied nor
  applied in a type position**

  Problem: `f[Id]()` over `comptime Id[T: AnyType] = T` and `def f[G:
  __generator_type[T: AnyType] AnyType]()` runs at the pin, where Mojito
  reports "cannot infer type parameter 'T' of 'Id' from the arguments".
  - `Checker::alias_generator` answers value and predicate bodies only: a
    type body (`AliasBody::Type`) read unapplied is no generator.
  - An application in a type position (`G[Int]`) resolves through
    `ty_from_anno`, which knows no value binder `G`. Upstream's is a type
    value over `#kgen.bind_params`: a `TyKind::Dependent` over a
    `ParamKind::BindParams` node, which `native::mono` folds per instance.
  - `Checker::generator_type` classifies a generator type's own list with
    no binder scope, so a result naming its own binders (`__generator_type[T:
    AnyType] List[T]`) does not resolve.
  - Probe: `size_of[G[Int]]()` over `Id` and `Pair[T: AnyType] =
    Tuple[Int, Int]` prints `8 16`.
  - Probe: `comptime L = G[Int]` in `build[G: __generator_type[T:
    Copyable] AnyType]`, called as `build[MkList]()` over `MkList[T:
    Copyable] = List[T]`, runs.
  - Probe: `var x: G[Int] = 5` over `Id` is rejected at the pin with
    "cannot implicitly convert 'IntLiteral[5]' value to 'G[Int]'", where
    Mojito reports "unknown type 'G'".
  - R1181 and R812 need it: their `tabulate` and `map_to_type` bodies apply
    type generators in type positions.
  - Found while landing R808 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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
  - Found while landing the callee shapes a signature applies (2026-10-06).
  - Depends on R386, which gives the parameter domain a value meta for a
    `Float64` and a `String`.
  - Model: Fable, Not Planned.

- [ ] **R1588 A `comptime assert` over a parameter that the pin folds to
  `False` does not end a return path**

  `def k[n: Int]() -> Int` whose `comptime if n == 1:` returns and whose
  `else:` holds `comptime assert n != n, "x"` is rejected ("'k' does not
  return a value on every path"), where the pin prints `1` for `k[1]()`.
  - `Checker::check_comptime_assert` records a failing assertion only
    when `fold_closed_constraint` decides its condition, which needs a
    condition over no parameter. The pin's parameter canonicalizer also
    folds an open one.
  - Probe: `n - n == 1` and `False and n == 2` behave the same.
  - The pin keeps `n == 2 and False` and `n < n` open (rejected there
    too), so the fold must match the canonicalizer, not three-valued
    logic.
  - Found while landing R1576 (2026-10-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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
  - The application holds the MIR declaration's name and every argument in
    one list (`ParamKind::Apply`, `param_expr.rs`), so the printer knows
    neither the source name nor which arguments are compile-time ones.
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
    power of two, got a type" instead.
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

- [ ] **R1220 A `TypeList` proposition over a pack in a signature type is
  rejected**

  Problem: `def f[*Vs: AnyType](x: B[Vs.contains[Int]()]) -> Int` over
  `struct B[b: Bool]` fails with "not a compile-time Int constant: not an
  associated comptime expression", where the pin prints the result.
  - `compile_dependent_ct_expr` (`checker/constraints.rs`) has an arm for
    a pack's length (`Vs.length == 2` runs) but none for a proposition,
    which a body's expression already records as a `ParamKind::PackQuery`.
  - Adding the arm is not enough: `ParamContext::replace_at` closes only
    `PackQuery::Length` against a bound pack, so the call then reports
    "expected B[TypeList[Vs.values]().contains[Int]()], found B[true]".
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
  - Found while landing R455 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1447 A struct parameter read through an instance has no field**

  Problem: `var t = T[3](); print(t.n)` over `struct T[n: Int]` reports
  "type 'T[3]' has no field 'n'", where the pin prints `3`.
  - `Checker::infer_member` (`checker/indexing.rs`) answers a member of an
    instance only from the struct's fields. Upstream resolves the name
    against the type's parameters too, so `t.n` is the instance's argument
    and `self.n` the binder, as `Self.n` is.
  - Probe: `self.f` in a method of `struct T[s: String, f: Float64]`, and
    `t.s` on an instance (pin: `2.5` and `a` for `T["a", 2.5]`).
  - Probe: a pack parameter read off an instance or its type, `len(r.Ts)`
    over a `_RegisterPackType[SIMD[DType.int32, 4], SIMD[DType.int32, 4]]`
    value `r`, or `len(P[Int, Bool].Ts)` over `struct P[*Ts: Movable]`,
    fails with "no overload matches" for `len`, where the pin prints `2`
    (found while landing R1547).
  - Probe: `t.k` over `t: Tag[idt(n)]` in `def g[n: Int]` reports "type
    'Tag[idt[Int](n)]' has no field 'k'", where the pin prints `1` for
    `g[1](Tag[idt(1)]())`; `t.key()` over a method returning `Self.k` runs.
  - Found while landing R1401 (2026-10-26).
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
  - `Dtype::UInt` (R1389) is the latest dtype added, with every table row,
    the VM's and Pliron's predicate arms included.
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
  - Restore `_uint_type_of_width`'s `_uint1`, `_uint2`, `_uint4`,
    `uint128`, and `uint256` arms in `stdlib/std/builtin/dtype.mojo`, and
    drop its second `comptime assert`.
  - Restore `SIMD.__hash__`'s `and Self.dtype != DType.float8_e8m0fnu`
    conjunct and its comment in `stdlib/std/simd.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R332 A `DType` float query as a type argument is rejected**

  Problem: `SIMD[DType.int32, DType.exponent_width[DType.float32]()](2)`
  prints its vector at the pin, while Mojito reports "not a compile-time Int
  constant: not an associated comptime expression".
  - The same query over a `DType` binder (`SIMD[DType.int32,
    DType.exponent_width[dt]()]` in `def lanes[dt: DType]()`) rejects the
    same way.
  - `eval_associated_ct` and `compile_dependent_ct_expr`
    (`checker/constraints.rs`) have no arm for the query. A closed one is
    its answer, and one over a binder is the application
    `ParamContext::dtype_float_query` builds, which the elaborator answers
    per instance.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R333 A value's `dtype` as a `DType` query argument is rejected**

  Problem: `DType.exponent_width[x.dtype]()` for `x: Scalar[dt]` prints `8`
  at `Float32` at the pin, while Mojito reports "not a valid SIMD element
  type: DType.dtype".
  - `dtype_from_arg` (`checker/annotations.rs`) reads only a
    `DType.<name>` spelling, a binder, or a type's `dtype`, not a value's.
  - A value's `dtype` is its type's lane slot, which the checker already
    records for a runtime read (`SemanticAdjustment::DtypeConstant`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R66 Small SIMD surface gaps the symbolic-lane probes found on concrete
  types**

  Problem: each of these runs at the pin on a concrete scalar or vector and
  is rejected by Mojito, so a symbolic template cannot license it either.
  - `len(v)` on a vector (`len_result_for_type` has no `Ty::Simd` arm),
    `abs` over a scalar alias (`is_numeric` excludes it),
    `Scalar[dt].MAX`/`.MIN`, and `**` on a lane.
  - Probe: `print(Int.MAX)`, `print(UInt8.MAX)`, and `T.MAX` over a module
    `comptime T = Int` stop at "Undefined variable 'Int'" (or 'UInt8'),
    a scalar type read as a value. The pin prints the maximum.
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

- [ ] **R1583 A call through a callable binder does not convert its
  parameter arguments**

  `func[4](x)` over `func: def[width: SIMDLength](SIMD[DType.int32,
  width]) thin -> SIMD[DType.int32, width]` bound to a generic `dbl` stops
  with "in `dbl`: unsupported value parameter `width` is not compile-time
  constant", where the pin prints `[2, 4, 6, 8]`.
  - With `width: Int` the same call runs, so the `4` reaches `dbl`'s
    instance unconverted: R1533 applies `SIMDLength`'s `@implicit
    __init__` to a direct call's struct value-parameter argument, and the
    callable-binder application skips that conversion.
  - Upstream's `SIMD.reduce` takes such a binder (`func: def[width:
    SIMDLength](SIMD[Self.dtype, width], SIMD[Self.dtype, width]) ->
    SIMD[Self.dtype, width]`, R1585).
  - Found while planning R1554 (2026-10-28).
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

- [ ] **R1210 A `comptime assert` message other than a string literal is
  rejected**

  Problem: `comptime assert n > 5, t"n is {n}"` fails `f[3]` at the pin
  with `constraint failed: n is 3`, while Mojito rejects the message as
  unsupported.
  - The pin evaluates the message per instance, as a compile-time
    `String` over the instance's parameters.
  - `GenericConstraint::WithMessage` carries a literal `String`, so the
    MIR `comptime.assert` has nowhere to put a message expression the
    elaborator would evaluate.
  - Probe: a t-string message, `t"n is {n}"`.
  - Probe: `String("n is ", n)` (`constraint failed: n is 3` at `f[3]`).
  - Probe: `"lit" + "eral"`, which runs at the pin.
  - Upstream's stdlib spells such messages at about 20 sites.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1470 A bare name of a non-integer parameter is rejected as a
  compile-time condition**

  `comptime if y:` over `y: Float64` in `def g[y: Float64]()` prints `y` at
  the pin, while Mojito rejects it with "a comptime if condition that
  compiles to no parameter expression".
  - Cause: the checked handoff holds one operation record per expression
    (`NodeFacts::adjustment`).
  - A name read as a parameter value keeps its `ParamValue` record there,
    and the condition's `ComptimeCondition` takes the same slot.
  - So the function lifted for the leaf's `Bool(x)` conversion could not
    read the name, and `Checker::converted_condition` lifts none for a bare
    name.
  - An integer leaf needs no function: it is `x != 0`, as `Int.__bool__`
    spells it.
  - Lever: give the condition record its own field beside the read's, as
    upstream's condition is the `__bool__` call's parameter expression over
    the read.
  - Probe: `comptime if Self.y:` over a struct's `y: Float64` parameter
    (the pin prints `S y`).
  - Probe: `comptime if x:` over `x: Scalar[DType.uint8]` (the pin prints
    `x` at 3 and `not x` at 0).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R77 A field's value cannot be read by reflection**

  Problem: `reflect[P].field_ref[0](p)`, a reference to field `0` of `p`,
  is rejected with "a parameterized type is not a value: expected a value,
  found the type 'reflect[…]'", where the pin prints the field.
  - `reflect` is a checker intrinsic. `Checker::reflection_query_of`
    (`checker/reflection.rs`) answers only argument-free queries
    (`field_count`, `field_names`, `field_types`, `field_index`).
  - Upstream's `field_ref[idx](ref s)` is a library method returning
    `ref[s]` the field (`lit.ref.struct.ger`), so in MIR it is a field
    projection of the argument's place at the field's name.
  - Upstream's `Equatable`, `Hashable`, and `Writable` defaults read fields
    through it. Mojito's `Hashable` and `Writable` defaults are synthesized
    (`comptime/synth.rs`), so no bundled body needs it yet (R1461).
  - Probe: `print(reflect[P].field_ref[0](p), reflect[P].field_ref[1](p))`
    over `P(3, 4)` (pin: `3 4`).
  - Probe: a generic `total[T](x: T)` summing `rebind[Int](r.field_ref[i](x))`
    under `comptime if types[i] == Int` in a `comptime for` reports
    "Undefined variable 'r'" (pin: the sum).
  - Probe: `reflect[P].field_ref[0](p) = 42` then `print(p.x)` (pin:
    `42`), now rejected at the same query.
  - Depends on nothing.
  - Model: Fable, Planned.

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

- [ ] **R1548 A local `comptime` initializer or a `materialize` operand
  that reads a run-time local is accepted**

  `var x = 3` then `comptime i = x + 1` prints `4`, where the pin rejects
  it with "cannot use a dynamic value in comptime initializer".
  - The checker's `StmtKind::Comptime` arm (`checker/statements.rs`)
    binds an initializer that denotes no parameter expression as a
    run-time immutable binding, without requiring it to read compile-time
    bindings alone (`Checker::reads_compile_time_alone`).
  - The arm already rejects an initializer reading
    `__is_run_in_comptime_interpreter` (R1546); the general rule subsumes
    that gate.
  - Probe: `comptime i = x and True` prints `True`; the pin: "cannot use a
    dynamic value in operator argument".
  - Probe: `comptime i = (x, 1)` then `print(i[0])` prints `3`; the pin:
    "cannot use a dynamic value in 'ref' argument".
  - A call over the local (`comptime i = twice(x)`) is already rejected,
    through `requested_binding`.
  - Probe: `var x = True` then `materialize[x]()` prints `True`; the pin:
    "cannot use a dynamic value in a parameter list". The `materialize`
    check (`Checker::infer_template_materialize`) rejects only an operand
    reading `__is_run_in_comptime_interpreter`.
  - `reads_compile_time_alone` already skips a name under a `type_of`
    operand, so gating on it keeps
    `assets/ok/type_of_value_annotation.mojo`'s
    `materialize[type_of(t) == String]()`.
  - Found while landing R1546 (2026-10-28).
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

- [ ] **R1507 A literal's type does not carry its value**

  Problem: `TyKind::IntLiteral`, `FloatLiteral`, and `StringLiteral` hold no
  value, and a `comptime N = 4` binding is typed `Int`, where upstream's
  literal types are value-parameterized (`IntLiteral[4]`).
  - Probe: `var y: type_of(N) = 4` over `comptime N = 4` prints `4` at the
    pin, `= {}` prints `4`, and `= 7` is "cannot implicitly convert
    'IntLiteral[7]' value to 'IntLiteral[4]'". Mojito rejects every
    `type_of` of a literal value as unsupported (`type_of(3)`, a module
    `FloatLiteral` constant in a field's type).
  - Probe: `comptime M: Int = 4` then `var z: type_of(M) = 9` prints `9` at
    the pin. Mojito cannot tell the annotated binding from `comptime N = 4`
    and rejects it too.
  - `SIMD`'s `IntLiteral` and `FloatLiteral` initializers
    (`simd.mojo:852`, `:972`) stay the checker's coercion until then: their
    bodies read `value.value` into `#pop.int_literal_convert` and
    `#pop<float_literal_convert>`, and `FloatLiteral` is not even a
    parameter type yet ("unknown type 'FloatLiteral'").
  - Porting either removes its literal from
    `Checker::binds_undeclared_overload` (`checker/call_inference.rs`).
  - Probe: `min(3, 2.5)` is rejected at overload resolution ("no overload
    matches"). The pin binds `std.math`'s `SIMD` overload at `Int` and fails
    the instance on the `FloatLiteral` initializer's assert ("the SIMD type
    must be floating point").
  - Probe: `def f(x: IntLiteral)` binds a run-time `IntLiteral` register
    rather than inferring `x.value` per call, so `f(n)` over `var n = 3`
    reports "expected Int, found Int" where the pin says "cannot be
    converted from 'Int' to 'IntLiteral[x.value]'"
    (`assets/type_error/int_literal_parameter_int_argument.mojo`).
  - Found while landing R1369 (2026-10-27).
  - Depends on nothing.
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

- [ ] **R272 A reflected field name materializes as `String`, not
  `StringSpan[ImmStaticOrigin]`**

  Problem: `materialize[names[i]]()` over `comptime names =
  reflect[T].field_names()` is a `String` in Mojito and a
  `StringSpan[ImmStaticOrigin]` at the pin, so the crossing diagnostic
  spells `Array[String, Int(2)]` where the pin spells
  `Array[StringSpan[ImmStaticOrigin], Int(2)]`.
  - The check answers `field_names()` with a list of plain compile-time
    strings (`ReflectQuery::answer`), which materialize as `String`
    literals.
  - A program that annotates the element as a `StringSpan` is rejected where
    the pin runs it.
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

- [ ] **R303 A capturing generic callable parameter applied at its own
  compile-time arguments fails to verify once its callee is inlined**

  Problem: with `invoke_captured` `@always_inline("nodebug")`,
  `inline-parametric` splices `callback[2](40)` into `main` as a
  `call.indirect` the verifier rejects ("indirect-call compile-time
  parameter metadata does not match its callable contract"); the pin runs
  it.
  - `def invoke_captured[origins: OriginSet, //, callback: def[n: Int](Int)
    capturing[origins] -> Int]()`, bound to an `@parameter` closure that
    captures a local.
  - Without `nodebug` the elaborator folds the closure and passes its
    environment, so `assets/ok/capturing_generic_callable_parameter_environment.mojo`
    runs.
  - So `canonicalize` folds no call that carries compile-time arguments.
  - Probe: `assets/ok/capturing_generic_callable_parameter_environment.mojo`
    with `invoke_captured` `@always_inline("nodebug")`.
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
  - The lever is a pack query over a list expression rather than over a
    pack binder (`ParamKind::PackQuery`).
  - Depends on nothing.
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
  - `declared_associated_type` (`mono/substitute.rs`) has no value
    binding to give `AssociatedParameter::Value`, which names its
    parameter by spelling.
  - The checker binds it by spelling too (`resolve_parameterized_member`).
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

- [ ] **R1563 `conforms_to` over a struct with a struct-valued argument
  stays unfolded in elaborated MIR**

  `comptime c = conforms_to(W[K(4)], Intable)`, with `@fieldwise_init
  struct K` and `struct W[k: K]`, stops with "keeps the parameter constant
  `conforms_to(W[K(4)], Intable)` in elaborated MIR", where the pin prints
  `False`.
  - The `comptime if conforms_to(W[K(4)], Sized)` form fails the same way.
  - The root is not located. The suspect is `Specializer::decide_type_conforms`
    (`mono/availability.rs`), whose `bind_struct_arguments` may not bind
    the `K(4)` construction argument, leaving the query undecided.
  - A `SIMD` instance's `SIMDLength` argument folds here, so the gap is the
    construction argument's, not the struct-valued slot's.
  - Found while landing R1550 (2026-10-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1639 A struct's type member read through an application over a
  symbolic parameter has no receiver**

  `T[t].current.answer()` inside `def f[t: Int]()`, over `struct T[n:
  Int]` with `comptime current = Self`, stops with "Undefined variable
  'T'", where the pin prints `3` for `f[3]()`.
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
  - Found while landing R1632 (2026-10-29).
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
- The `cfd79ca736` re-pin (2026-10-17, Mojo `1.2.0.dev2026101705`) closed
  the spelling changes its window forced: `match`, `case`, `class`, `del`,
  and `yield` are keywords, `std.hashlib` is `std.hash`, and
  `FileDescriptor` is `RawFileDescriptor`. The window's unimplemented
  features are R1114–R1127, and its two upstream regressions are probes
  (`for_else_over_range.mojo`, `tuple_explicit_pack_application.mojo`)
  re-run at the next re-pin, not entries here.

Behavioral divergences from the pinned Mojo, to burn to zero. Every new
divergence lands here with a probe or a `cases.tsv` `mojito-only` /
`output-diff` row, and leaves when its probe promotes to an `assets/ok`
fixture. Each entry's `Ledger name` is the slug `conformance/cases.tsv` uses
for it, and an entry that waits on another names it. Five divergences are
retained on purpose and re-probed rather than fixed; they are listed in
[`docs/non-goals.md`](non-goals.md).

- [ ] **R1624 An integer literal's `//` or `%` by zero is an error, where
  the pin folds it to `0`**

  `print(10 // 0, 10 % 0, -7 // 0)` traps, and `comptime c = 10 // 0` fails
  its compile-time evaluation, where the pin prints `0 0 0` and `0`.
  - Upstream's `IntLiteral.__floordiv__`/`__mod__` fold through
    `int_literal_bin`, which yields `0` for a zero divisor, as `SIMD`'s
    `__floordiv__` masks one.
  - Mojito's `IntLiteral::floor_div`/`floor_mod` (`mojito-common`) return
    `None` there, so `param_expr::fold`'s `fold_int_literal` reports an
    arithmetic error and the VM's literal division traps.
  - Ledger name: `assets/runtime_error/div_by_zero.mojo`, which moves to
    `assets/ok` when this lands, with its errors-ledger and manifest rows.
  - Also R1538's `comptime g = x // 0` probe, whose `x` stays a literal.
  - `docs/non-goals.md` no longer keeps a compile-time zero divisor an
    error: `SIMD.__floordiv__`'s mask makes the `Int` form `0`, and this
    entry makes the literal form follow `int_literal_bin`.
  - Found while landing R1566 (2026-10-29).
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

- [ ] **R1587 An instance drops the `comptime assert`s after a selected
  `comptime if` arm that never falls through**

  `def k[n: Int]() -> Int` holding `comptime if n == 1: return 1`, then
  `comptime assert n == 5, "never"` and `return 3`, prints `1` for
  `k[1]()`, where the pin fails the instance: "constraint failed: never".
  - `splice_comptime_arm` (`native/mono/select.rs`) discards the block's
    tail when the taken arm returns, before `discharge_comptime_asserts`
    judges what the instance keeps. The pin judges the tail's assertions.
  - Probe: `comptime assert False, "never"` after the same `comptime if`,
    with no trailing `return`, runs since R1576 and fails at the pin.
  - A tail after a plain `return` is erased before either judges it
    (`hir::Lower`'s `live_statements`, the pin's parser), so it stays
    unjudged.
  - Found while landing R1576 (2026-10-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1537 An `@implicit` constructor's parameter admits a literal by
  coercion, not at the literal's default type**

  `var y: F = 2`, where `F`'s only `@implicit __init__(out self, value:
  Float64, /)` stores `value + 0.5`, prints `2.5`, where the pin rejects it:
  "cannot implicitly convert 'IntLiteral[2]' value to 'F'".
  - Cause: `Checker::implicit_conversion_constructor` (`checker.rs`) keeps
    a constructor whose parameter the source `coerces` to, so a literal
    reaches any scalar it materializes as. The pin converts a literal only
    at its default type (`Int`, `Float64`, `String`) before an `@implicit`
    constructor.
  - Probe: `f[C]()` over `def f[w: F]()` with `comptime C = 2` prints
    `2.5`; the pin rejects it ("value has type 'IntLiteral[2]'"). The
    parameter conversion inherits the rule (R1533).
  - Probe: `f[3]()` into an `@implicit __init__(value: UInt8)` is rejected
    as unsupported, where the pin rejects it as a type mismatch.
  - Probe: the other way round, `var s: S = "abc"` over `@implicit
    __init__(out self, value: String, /)` storing `value.byte_length()` is
    rejected ("expected S, found StringLiteral"), where the pin prints `3`:
    `StringLiteral` does not coerce to `String`. `f["abc"]()` over `def
    f[w: S]()` (pin `3`) fails the same way ("type mismatch for value
    parameter 'w': expected S, found StringLiteral").
  - Found while landing R1533 (2026-10-27).
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

- [ ] **R1530 A closed list display's element folds in a type**

  `SIMD[DType.int8, [2, 1][0]]` is `SIMD[DType.int8, 2]` in Mojito, so `def
  f(x: SIMD[DType.int8, [2, 1][0]]) -> SIMD[DType.int8, 2]: return x` runs,
  where the pin rejects it, naming the unfolded `(load_from_mem
  Array[__literal_size__=Int(2)](Int(2), Int(1),
  __list_literal__=NoneType(None)).__getitem_param__[Int(0)]())`.
  - `build_ct`'s `Index { object: ListLit }` arm (`checker/constraints.rs`)
    folds `param_list.get` over the closed display.
  - The lever: delete that arm once a display is its constructor's
    application (R1529), so the read is the display's `__getitem_param__`.
    Withdrawing it may need upstream's `load_from_mem` reading of
    `__getitem_param__` over an application; the session that takes it
    decides.
  - Found while planning R1472 (2026-10-27).
  - Depends on R1529.
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
    '__module$std$math$max'" (`call_inference.rs` passes the linked name to
    `TypeError::BadCall`), where the pin says "no matching function in call
    to 'max'" (`assets/type_error/max_literal_first_mismatch.mojo`).
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
  - Fix: respell the bundled `Int`, `Bool`, and float `write_to`s after
    upstream's, then promote a probe to an `assets/ok` fixture.
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

- [ ] **R712 The scalar operation traits `Addable`, `Negatable`, and kin
  are accepted as bounds the pin does not declare**

  Problem: `def g[T: Addable](a: T)` compiles in Mojito, where the pin
  rejects it with `use of unknown declaration 'Addable'`.
  - The same holds for `Subtractable`, `Multipliable`, `Negatable`, and
    `Andable`, probed at the pin; `Powable`, `Intable`, `Boolable`,
    `Comparable`, and `Equatable` are the pin's own.
  - Cause: the compiler-known operation traits
    (`mojito_types::conformance`, `infix_operation_trait` and
    `prefix_operation_trait` in `checker/builtins.rs`) are declared
    whether or not upstream's library has them.
  - Each Mojito-only name should leave the compiler-known list, keeping
    only the traits the pin's stdlib declares.
  - Since R1358 an operator through a bound is its requirement's call, so
    the erased arithmetic arm of `infer_infix` and the `param_has_bound`
    arm of `infer_prefix` (`checker/operators.rs`) serve only these traits
    (and `not x` through `Boolable`); they go with them.
  - `docs/features.md`'s scalar-operation-traits row names them as
    supported bounds.
  - Since R1552 the VM's and Pliron's `unary invert` arms over `Int`,
    `UInt`, and integer `SIMD` lanes (`runtime.rs` `apply_prefix`,
    `lower/simd.rs`, `lower/consts.rs`) serve only these erased bounds:
    they go with them. The `binary` arms stay, since `pop` ops lower to
    them.
  - Found while landing R709 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R860 A `TrivialRegisterPassable` struct may define a copy
  constructor or `__del__`, or hold a non-trivial field**

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
  - `Span`'s is R1226.
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

- [ ] **R579 `Representable` resolves as a trait, where the pin does not
  declare it**

  Problem: `conforms_to(T, Representable)` compiles in Mojito with no
  import. The pin rejects it as an unknown declaration.
  - Cause: the checker resolves any name on its compiler-known trait list
    (`mojito_types::conformance::COMPILER_TRAITS`) whether or not the
    program imports it.
  - The fix is to resolve a compiler-known trait only where upstream's
    prelude or an import puts its name in scope.
  - Pinned by `conformance/probes/compiler_trait_name_without_import.mojo`.
  - Found while landing R564 (2026-10-10).
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
  - `simd_floordiv_mod.mojo` differs only at `MIN // -1`, a lane the pin's
    compile-time fold makes `0` and Mojito leaves `MIN`. At run time the
    pin prints `-128` for an `Int8` lane, as Mojito does, and crashes
    (SIGFPE) on `Int`, so the fixture's row pins LLVM's undefined case,
    not a zero-divisor rule.
  - R1567 (`pliron_float_pow.mojo`: `_powf_scalar`'s `exp(log)`) removes
    a row.
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

- [ ] **R176 Capturing callables are accepted as runtime arguments, typed
  locals and `mut self` conformers, where the pin takes them only as
  compile-time parameters**

  Problem: three callable-shape leniencies. Mojito passes a capturing
  closure as a runtime argument, binds one to a local under an explicit
  `capturing[...]` annotation, and accepts a `mut self` `__call__` as a
  `def(...)` conformer.
  - A capture-list closure's or capturing lambda's value is its storage
    struct (R1378), which runs where the pin runs it: an untyped local,
    a generic container, a `F: def(...)` binder. Only these three
    leniencies convert it to the closure of its body
    (`SemanticAdjustment::ClosureCallable`).
  - The pin rejects the conversion: `call_rt(inner, 1)` over `def
    call_rt(f: def(Int) capturing[_] -> Int, a: Int)` reports "depends on
    an unresolved parameter '__origins__'", where Mojito prints `11`.
  - The pin wants a read receiver on `__call__`.
  - Three corpus fixtures moved to `assets/extensions/`.
  - Three more were respelled onto `thin` contracts and `@parameter def`
    arguments.
  - The bundled library's own run-time handler parameters come first
    (R1433).
  - Ledger name: `capturing-lambda-argument`, `capturing-lambda-locals`,
    `owned-capture-closure-locals`, `mut-self-callable-struct`.
  - Depends on R1433.
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
    So does `OwnedPointer.unsafe_ptr`, whose bundled return is also
    interior where upstream's is `Pointer[T, origin]`
    (`assets/ok/owned_pointer_api.mojo` prints `drop 9` before `held 9`;
    `conformance/assets-mojo-output-diffs.tsv`, family `drop-timing`).
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
  - Port upstream's `SIMD.__truediv__` and `__rtruediv__` (`pop.div`) and
    route `/` over a `SIMD` to them, as `+` is routed.
  - The run-time half moves no corpus fixture (measured while planning
    R1551).
  - The compile-time half is `param_expr::fold`'s `Div` over `Int`
    operands, which must truncate as `pop.div` does: `comptime y = x / 2`
    over `comptime x: Int = 7` prints `3` at the pin.
  - Port `__itruediv__` (`simd.mojo:1709`) with `__truediv__`, so `/=`
    over a `SIMD` calls it, as R1552 routes `+=`.
  - Pliron's `lower_true_div` and the VM's `Div` arm of `numeric_op`
    retire.
  - Depends on nothing.
  - Model: Fable, Planned.

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

- [ ] **R423 A writer overriding `write` misses the nested `write` a
  scalar's `write_to` makes at the pin**

  Problem: a `Writer` whose own `write` wraps each argument
  (`self.text += "<"`, then `args[i].write_to(self)`) prints `<1x>` for
  `b.write(1, "x")` in Mojito and `<<>1x>` at the pin.
  - The pin's `Int.write_to` and `Float64.write_to` are Mojo bodies that
    call `writer.write` for their digits, which re-enters the override.
  - Mojito's checker marks `x.write_to(w)` on a builtin scalar
    `SemanticAdjustment::InvertedWrite`, and MIR lowers it
    (`mir/lower_expr/expr_method.rs`) to a `$write_formatted` method call
    on the writer (`mojito_symbol::symbol::WRITE_FORMATTED`).
  - The VM answers that call by formatting the value in the host
    (`format_value` in `backend/vm/invoke.rs`) and calling the writer's
    `write_string`, and Pliron's `lower/methods.rs` does the same, so the
    override is never re-entered.
  - Pinned by `conformance/probes/writer_override_scalar_write_to.mojo`.
  - Ledger name: `writer-override-scalar-write-to`.
  - Found while landing R414 (2026-10-06).
  - Depends on R276, which moves the scalar text into Mojo bodies.
  - Model: Opus, Not Planned.

- [ ] **R1462 A `write_to` through a `Writable` bound is a host-formatted
  write, where upstream calls the conformer's witness**

  Problem: `x.write_to(w)` over `T: Writable` is checked as an inverted
  write (`SemanticAdjustment::InvertedWrite`,
  `checker/method_calls/intrinsic_receivers.rs`), which formats `x` and
  hands the text to `w.write_string`, so a writer overriding `write` is not
  re-entered by the conformer's own `writer.write(...)`.
  - Probe: `show[T: Writable](x: T, mut w: Wrap)` calling `x.write_to(w)`
    over a struct whose `write_to` is `writer.write("P")`, with `Wrap.write`
    wrapping its arguments in `<`/`>`, leaves `P` where the pin leaves
    `<P>`, on both backends. The direct call `P(1).write_to(w)` matches.
  - A call through the bound needs every type that can bind `T` to answer
    `write_to` as a witness, which the scalars, `Bool`, and the literals do
    only once their text is Mojo (R276).
  - Found while landing R424 (2026-10-26).
  - Depends on R276.
  - Model: Opus, Not Planned.

- [ ] **R1463 A `Pointer` conforms to `Writable` but cannot be printed**

  Problem: `print(Pointer(to=x))` is rejected ("does not conform to trait
  'Writable'"), where the pin prints the address, though
  `modelled_declares` answers `Writable` for `TyKind::Pointer`, as
  upstream's `Pointer` declares.
  - The checker's `printable_argument` (`checker/builtins.rs`) does not
    take a pointer, the VM's `format_value` writes `Pointer(0x11+0)`, and the
    native lowering has no display of one.
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
  - Mojito's stand-in, `_FormatUtils.format_literal` in
    `stdlib/std/string.mojo`, parses at run time and aborts with the same
    message; a path the run never takes is accepted.
  - Ledger: `format-literal-template-runtime`
    (`conformance/fixtures/format_literal_template_runtime.mojo`).
  - The stand-in and the checker's `LiteralFormat` adjustment go away when
    `StringLiteral` declares upstream's `format`.
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
  - Ledger name: `array-negative-literal-index`.
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

- [ ] **R1435 A lambda expression is a temporary of its closure's storage,
  where the pin's is an lvalue of its storage variable**

  `var peek = lambda {var token^} -> Int: token.id` over a `Movable`
  `Token` runs in Mojito; the pin rejects it ("value of type 'def() -> Int'
  cannot be implicitly copied, it does not conform to
  'ImplicitlyCopyable'"), because upstream's `emitClosure` returns its
  storage variable, which a binding or an owned argument copies.
  - Mojito builds the storage as a call result (`lambda_storage_value`,
    `mir/lower_expr/calls.rs`), which a binding moves.
  - `check_callable_display_element` (`checker/inference.rs`) stands in
    for the copy in a display, rejecting a lambda element whose moved
    capture is not `ImplicitlyCopyable`
    (`assets/type_error/capturing_lambda_display_transferred_uncopyable.mojo`);
    drop that rule once the lambda is an lvalue.
  - Probe: the `var peek = lambda {var token^} ...` binding above.
  - Probe: `transform(lambda (x: Int) {k} -> Int: x * k, 3)` passes the
    storage, a call result in a register, to a `read` parameter and
    destroys it there, which R1445's rule rejects
    (`assets/ok/capturing_lambda_storage.mojo`).
  - Found while landing R1378 (2026-10-25).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1433 The bundled handler APIs take run-time `def(...)
  capturing[_]` arguments, where upstream's take closure-trait arguments**

  `List.deinit_with`, `Optional.map`, `and_then`, `deinit_with` and
  `__init__(init_with=)`, `Set.clear_with`, `Dict.deinit_with` and
  `reap_with`, and `Array.deinit_with` take a run-time callable, so a
  closure passed to one converts to the closure of its body
  (`SemanticAdjustment::ClosureCallable`), where upstream takes the
  storage through a closure trait (`deinit_func: Some[def(var Self.T)]`,
  `map[To: Movable, //, Mapper: def(var Self.T) -> To]`).
  - Port each signature and argument name (`deinit_func`, `destroy_func`,
    `Mapper`) in `stdlib/std/collections/` to upstream's.
  - `Some[def(...)]` is "unknown type 'Some'" today (R813), so spell an
    explicit binder until it lands.
  - Probe: `l^.deinit_with(h)` over `def h(var e: Int) {mut total}` and
    `[1, 2, 3]` prints `6` on both, through a converted closure in Mojito.
  - Found while landing R1378 (2026-10-25).
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

- [ ] **R1352 `repr` of an `Error` prints its bare message**

  `print(repr(Error("boom")))` prints `boom` on the VM and is rejected
  natively ("unsupported type `Error`"), where the pin prints
  `Error('boom')`.
  - Upstream's `Error` is a library struct whose `write_repr_to` writes
    the type name around the quoted message. Mojito's is the intrinsic
    `TyKind::Error`, so `repr` is hand-rolled per backend.
  - The VM's `repr` formats it as `str` does (`format_value` in
    `crates/mojito-vm/src/backend/vm/dispatch.rs`), and Pliron's
    `lower_repr_builtin` (`lower/methods.rs`) has no `Error` arm.
  - Found while landing R1326 (2026-10-24).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1482 The bundled `List` stores three fields where upstream's
  stores two**

  `reflect[List[Int]].field_count()` prints `3`, where the pin prints `2`:
  the bundled `List` holds `data: UnsafePointer[T]`, `size`, and `cap`,
  where upstream's holds `_storage: Allocation[T]` (the buffer and its
  capacity) and `_len`, so every reflected read of a `List` (field names,
  types, indexes) answers another struct.
  - Porting it needs upstream's `Allocation` and a rewrite of every
    bundled `List` member over the two fields.
  - The VM reads a `List`-shaped value's `data`, `size`, and `cap` by name
    (`backend/vm/values.rs`), which moves with it.
  - Found while landing R1383 (2026-10-26).
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

- [ ] **R1523 An `@always_inline("builtin")` body the pin rejects is
  accepted outside the fold's domain**

  The pin judges every builtin body by its fold, where Mojito judges only a
  body in the compile-time call domain (`applicable_declarations`: scalar
  parameters by value, a declared result) and only the constructs
  `builtin_body_rejection` lists.
  - A body the fold cannot read (a host-implemented callee, a `Bool`
    conversion) keeps its call an application instead of being rejected.
  - Probe: `def g(mut n: Int): n += 1` under the decorator runs, where the
    pin says "does not support this argument convention"; so does `def
    g(n: Int, s: String) -> Int: return n + n`.
  - Probe: `print(n)` before `return n + n` runs, where the pin says "does
    not support MLIR operation lit.ref.immut"; so does `return Int(b)` over
    a `b: Bool`.
  - Probe: `return min(n, 3)` runs, where the pin says "only supports calls
    to other '@always_inline("builtin")' functions": `min` is
    `std.math`'s library overloads, and `calls_ordinary_function` asks
    only `applicable_functions`, the compile-time call domain, which holds
    neither overload.
  - Probe: `return L(n)` or `return Self(n)` in a builtin static method of
    a `@fieldwise_init struct L` runs, where the pin says "only supports
    calls to other '@always_inline("builtin")' functions":
    `calls_ordinary_function` exempts every construction, where the pin
    admits only a builtin `__init__`.
  - Found while landing R1395 (2026-10-27).
  - Depends on R1522.
  - Model: Opus, Not Planned.

- [ ] **R1527 A run-time call of an `@always_inline("builtin")` callable
  judges its `comptime assert`**

  `checked[-1](1)` over a builtin `def checked[k: Int](n: Int) -> Int`
  opening with `comptime assert k > 0, "positive"` prints `2` at the pin,
  where Mojito fails: "function instantiation of `checked` failed:
  constraint failed: positive".
  - The pin folds every call of a builtin callable at the call, and its
    fold drops the assertion. `builtin_fold.rs` drops it from the
    parameter-position template only, so a run-time call instantiates the
    body and the elaborator judges it.
  - Probe: `G[-1].checked(1)` over a builtin static method of `struct
    G[k: Int]` asserting `Self.k > 0` behaves the same.
  - Probe: `var x = a < SIMD[DType.int32, 4](2)` over a `comptime a =
    SIMD[DType.int32, 4](1, 2, 3, 4)` prints `True` at the pin, which folds
    the builtin `SIMD.__lt__` and drops its one-lane assertion. Mojito
    fails the instance with "Strict inequality is only defined for
    `Scalar`s": the check records the assertion at every run-time
    position (`Checker::record_instance_assertion`), and only a
    `comptime` position skips it.
  - Without the decorator both are rejected, at the pin and here.
  - Found while landing R1522 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1565 A method's compile-time parameter may reuse its struct's
  parameter name**

  `struct H[d: DType]` with `def f[d: Int]()` checks and runs, where the pin
  rejects the method with "invalid redefinition of 'd'".
  - The method binder shadows the struct's inside the method, where the
    pin's scope holds both and refuses the second.
  - Reject the method binder at its declaration, as the pin does, and add
    an `assets/type_error` fixture with its `# expect:`.
  - Found while landing R1550 (2026-10-28).
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

- [ ] **R1418 A loan anchor or borrow slot binds a memory-only call
  result a second time, where upstream borrows its call-result slot**

  Upstream gives a memory-only call result one `__call_result_tmp__` at the
  call, and every borrower's loans sit on that storage.
  - Mojito's `$arg_loan_r` (`anchor_borrowing_argument`), `$view_recv_r`
    (`anchor_borrowing_temporary`), `$conv_view_r`, and `$mat_r`
    (`materialize_borrow_slot` and its copies in `mir.rs` and
    `lower_stmt.rs`) each bind the temporary afresh, so
    `Flatten::call_result` (`mir/call_results.rs`) skips a temporary one of
    them carries loans for.
  - An origin-bearing view at a call argument stays in its register
    (`Flatten::operand_expr_c`), whose provenance keeps the viewed storage
    live, so it gets no slot at all.
  - Fix: register the call-result slot as the owner's var and put the
    anchor's `EstablishLoans` on it. A register-passable source keeps its
    own `$mat_r`, upstream's `anonymous` materialization.
  - Depends on R1226.
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

- [ ] **R1425 A MIR call passes a by-address argument as a loaded value
  beside its place, where upstream's call takes the address and loads
  nothing**

  `MirInstr::Call` takes every operand as a value register, so a memory-only
  `read` argument or a `mut`/`ref` one is a whole load of its place, with
  the place itself retained in `arg_places`.
  - The VM replaces the loaded value with a handle to the place, and
    Pliron recovers the address from the place
    (`place_backed_arg_value`, `crates/mojito-pliron/src/lower/abi.rs`), so
    the load reads nothing a backend uses.
  - `cse` cannot merge two such loads, because their memory-only result
    may own what a consuming reader takes.
  - Probe: `def twin(p: Pt) -> Bool: return same(p, p)` over
    `@fieldwise_init struct Pt(Copyable, Movable): var x: Int; var y: Int`
    keeps two adjacent whole `place.load`s of `p` in concrete `twin`
    (with `@no_inline` on both), where upstream passes `p`'s address twice.
  - Probe: `bump(p)` over `def bump(mut a: Pt)` spells a `ref.make` and a
    whole `ref.read` of `p` before the call.
  - Probe: a memory-only temporary at a `read` parameter has no place at
    all, so a write through the callee's `read self` lands on the callee's
    copy: `twice(lambda {var s^} -> Int: (s.__iadd__("!"),
    s.byte_length())[1])` over a `String`, with `F: def() -> Int`, prints
    the pin's `34`, but stops on "double free of Pointer allocation" on the
    VM with `AUTOMATIC_INLINE` removed from `post_elaboration.rs`.
  - Fix: pass the address as the operand, a `ref.make` of the place, and
    teach the inliner's parameter binding, `sroa`, and `mem-2-reg` to read
    it as an address.
  - Depends on nothing.
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

- [ ] **R1440 A reference result passed to a `read` parameter is copied,
  where upstream binds the parameter to the reference**

  `show(o.value())` over `def show(s: String)` and an `Optional[String]`
  copies the `String` (`String.__copyinit__` into a register), passes the
  copy, and destroys it after the call, where upstream's `read` argument is
  the reference `value()` returned.
  - Cause: the checker treats a reference result's ordinary value use as
    owning an independent value (`Checker::check_reference_result_reads`),
    so MIR lowers it to a `ReadRef` and a `CopyValue`, even when the
    consumer is a `read` parameter that only borrows it.
  - So a memory-only value lives in a register that a `deinit` call on it
    destroys, which R1445's verifier rule rejects.
  - Probe: `isdir(default_dir.value())` in
    `assets/ok/tempfile_mkdtemp.mojo` (a generic `read` parameter).
  - Probe: `s.append(c)` over a `Codepoint` read through a reference
    (`assets/ok/nominal_string_bytes_capacity.mojo`,
    `nominal_string_bytes_peek_split.mojo`, `view_temporary_origins.mojo`).
  - Probe: `print(o.value())` and its kin, a builtin's argument too
    (R1441): `optional_string_from_stringify.mojo`,
    `template_method_reference_result.mojo`,
    `module_reference_result_reads.mojo`,
    `always_inline_nodebug_reference_results.mojo`,
    `origin_binder_result_keeps_argument.mojo`, `ctfe_optional_result.mojo`,
    `match_enum_cases_optional.mojo`, `pack_element_rebind.mojo`, and the
    `template_method_*` reference fixtures, all under `assets/ok/`.
  - Found while landing R1356 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1441 A builtin call's memory-only argument reaches it in a
  register, where upstream's library function reads it in place**

  `print`, `input`, `abort`'s `_mojito_abort`, `Error(...)`, `Int(...)`,
  `divmod`, `abs`, and `round` are checked and lowered as builtins
  (`checker/call_inference.rs`), so no argument binds a `read` parameter
  and MIR lowers each as a plain value.
  - So a memory-only argument is a register: a copy of a `read` parameter,
    a moved value, or a call or subscript result kept in its register
    rather than its call-result slot, which a `deinit` call then destroys.
  - Upstream declares each as a library function whose `read` parameters
    bind a place or a `__call_result_tmp__`.
  - Lever: record a read convention per builtin argument
    (`BorrowReadArgument` / `ReadTemporaryArgument`), so MIR lowers it
    through `Flatten::read_operand` (`mir/call_results.rs`). R1287 makes
    `print` itself a library function.
  - Probe: `print(s[byte=1:3])` and kin (`assets/ok/nominal_string_graphemes.mojo`,
    `nominal_string_keyword_slices.mojo`, `slice_literal_constructor_binding.mojo`).
  - Probe: `print(xs[::2])` over a `List[Int]` (`assets/ok/slice.mojo`,
    `slice_none_bound.mojo`, `list_strided_slice_normalizes.mojo`,
    `pliron_slice_desc.mojo`), and `Int(s[0])` over a `Codepoint`
    (`nominal_string_indexing.mojo`).
  - Probe: `input(prompt)` (`assets/ok/input.mojo`,
    `pliron_input_echo.mojo`) and `abort(msg)` (`with_statement.mojo`,
    `with_multiple.mojo`, `file_handle_roundtrip.mojo`,
    `file_descriptor_stdout_order.mojo`) copy their `String` parameter.
  - Probe: `raise Error(message)` in the bundled `atof` moves its `String`
    into a register (`nominal_string_parse.mojo`,
    `nominal_string_parse_errors.mojo`, `nominal_string_atof_rounding.mojo`,
    `list_unsafe_uninit_length.mojo`).
  - Probe: `divmod(a, b)`, `round(x)`, and `abs(x)` over a generic `T` copy
    their operands (`boolable_and_divmod.mojo`,
    `numeric_operation_traits.mojo`), a struct's too since R1361
    (`math_operation_traits_library.mojo`); R1458 closes it.
  - Found while landing R1356 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1442 A t-string, conversion, subscript-index, or trait-dispatched
  receiver operand keeps a memory-only call result in its register**

  These consumers lower their operand with `Flatten::expr`, not the
  call-result path (`expr_c`, `operand_c`, `take_c` in
  `mir/call_results.rs`), so a result the checker marked `IndirectResult`
  never reaches its `$call_result_tmp_r` slot.
  - Upstream's call emission writes every memory-only result to its
    destination or a `__call_result_tmp__`, whatever consumes it.
  - Probe: `repr(x)` as a t-string or `format` operand, a
    `$write_formatted` argument (`assets/ok/generic_reads_copy_nothing.mojo`,
    `parameter_list_of_alias.mojo`, `parameter_list_raw_values.mojo`,
    `path_operations.mojo`, `string_format_declared.mojo`,
    `value_pack_parameter_list_writable.mojo`).
  - Probe: a `Slice` call result or moved `Slice` converted to
    `StridedSlice` (`slice_kind_construction.mojo`,
    `slice_library_struct.mojo`, `slice_descriptor_protocols.mojo`), and a
    `StridedSlice(...)` subscript index (`slice_descriptor_protocols.mojo`).
  - Probe: an implicitly converted argument in a generic method,
    `self.keep(x)` building a `Wrapper[T]`
    (`template_method_converting_call_argument.mojo`).
  - Probe: `pk(x).release()` over a generic `-> T`, an owned receiver
    through a trait bound (`comptime_if_arm_destroy_joins.mojo`,
    `through`).
  - Found while landing R1356 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1443 An implicitly copied consuming receiver is copied into a
  register, where upstream copies it into a temporary the call consumes**

  `v.unwrap[Int]()` over an `ImplicitlyCopyable` `Variant[Int, String]`
  copies `v` (`var.use copy`, or `copy_consuming_receiver` in `mir.rs`)
  and hands the `deinit` receiver the copy's register, with no place.
  - Upstream's owned argument is the address of the copy's temporary,
    which the callee consumes.
  - Lever: copy into a call-result slot and pass its place as the
    receiver place, which the call consumes, as an explicit destroy's
    receiver place is. Both method-call paths drop the place of a
    copied receiver (`ordinary_method_call`, `parameterized_method_call`).
  - Probe: `v.unwrap[Int]()`, `w.unsafe_unwrap[Int]()`, and
    `x.deinit_with(h)` (`assets/ok/variant_owning_api.mojo`).
  - Probe: `t.reverse()` and `t.concat(u)` over a `Tuple[Int, String]`
    (`tuple_template_served.mojo`), and `self.x.or_else(d)` over an
    `Optional[T]` (`template_method_copied_consuming_receiver.mojo`).
  - Found while landing R1356 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1444 A callable binder's value is copied into a register to be
  called or handed to a `Variant` handler**

  `Variant.__init__(init_with=f)` and `Variant.deinit_with(f)` copy their
  `F` parameter (`var.use copy`) into the `call.indirect` or the
  storage's handler operand (`variant.deinit_with`), then destroy the
  copy by a `drop.reg`.
  - `F` is a type parameter, so memory-only, as upstream's default is.
  - Upstream calls a callable binder through its `__call__`, which borrows
    it, and passes the handler by reference.
  - Probe: `assets/ok/variant_init_with_constructor.mojo` and
    `variant_deinit_with_linear_payload.mojo`.
  - Probe (from R1109): the handler operand runs a struct handler's
    `__call__` from its type (`mir::TYPE_REACHED_MEMBERS`), so a
    `__call__` ignoring an argument keeps it; upstream's is a call
    `RemoveUnusedParams` rewrites.
  - Found while landing R1356 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1445 The MIR verifier does not reject a destroy of a memory-only
  register**

  `verify/drops.rs` accepts a `drop.reg`, or the `deinit`-receiver call
  `lower_drops` turns it into, of any owned register, where upstream's
  `CheckLifetimes` tracks only register-passable values as SSA values
  (`numValueBits = 1` in `ValueSet::addValue`) and destroys a memory-only
  one through its storage.
  - The rule: reject a `drop.reg`, and a `deinit`-receiver call with no
    receiver place, whose register type
    `MirDeclarations::register_passable` answers no for.
  - Exempt a receiver a forwarding `var.use` defines: that is the slot's
    storage handed over, the form `lower_drops` gives every slot drop.
  - A 2026-10-26 census of `assets/ok` with the rule on found 58
    fixtures, every one a probe of R1440 through R1444, R1435
    (`capturing_lambda_storage.mojo`), or R1095
    (`trait_comptime_requirement_default.mojo`).
  - Re-run the census over `assets/ok` and `assets/ownership_ok`, with
    `MOJITO_PRE_ELABORATION=0` too, before landing it: the rule runs
    after every pass in a debug build.
  - Depends on R1440, R1441, R1442, R1443, R1444, R1435, R1095.
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

- [ ] **R1458 `abs`, `round`, and `divmod` are compiler builtins, and
  `pow` is missing, where upstream's `std/math` declares library functions**

  `round(2.567, 2)` is rejected ("'round' expects 1 argument(s), got 2")
  and `pow(2, 10)` is an undefined variable, where the pin prints `2.57`
  and `1024`.
  - Upstream declares `abs[T: Absable](value: T)`, `round[T:
    Roundable](number: T)` and its `ndigits` overload, `divmod[T:
    DivModable]`, and `pow[T: Powable]` beside the traits, each returning
    the requirement's result.
  - Mojito checks the first three by name (`Checker::infer_abs`,
    `infer_round`, `infer_divmod`), the backends run them as builtins, and
    the elaborator rewrites a struct operand into the dunder call.
  - Declare the four in `stdlib/std/math.mojo`, exported from the prelude,
    and delete the builtins. The scalars already answer each requirement
    as a method.
  - Closes R1441's `divmod`/`round`/`abs` probe: a `read` parameter binds
    the argument in place.
  - Probe: `divmod(UInt8(7), 3)` is rejected ("'divmod' is not defined for
    UInt8 and Int"), where the pin prints `(2, 1)`: `builtin_args`
    materializes the literal to `Int` before the pair is typed, where
    upstream's `divmod[T: DivModable]` binds `T` from `UInt8`.
  - Found while landing R1361 (2026-10-26).
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

- [ ] **R600 An overloaded declaration's symbol is spelled from its
  syntax, and its call's from its checked type**

  The declaration side re-spells an overloaded `def` or method's symbol
  from its AST signature, while the call side spells it from the checked
  `FuncTy`, and upstream mangles a declaration once, from its resolved
  signature.
  - `lowered_def_name`/`lowered_method_name` (`mojito-symbol`) run
    `signature_from_ast`, which still folds an annotation's own arithmetic
    (`SIMD[d, N * 2]`) over the check's constants.
  - The call side is `overload_support::callable_lowered_name`/
    `method_lowered_name`; the two agree only because both spell alike.
  - The check needs the owner before the signature is typed
    (`check_def` classifies binders under it), so the lever restructures
    binder ownership: type the signature, then name its declaration.
  - Found while landing R599 (2026-10-10).
  - Probe: `def pick(p: Pair[Point])` beside `def pick(n: Int)` fails on
    the VM with "does not support … `pick$ov$Pair$Point`", where the pin
    prints `4` for `pick(Pair[Point](Point(4)))`. The declaration spells
    the bare operand `VPoint` (`symbol::value_expr_raw`), the call
    `Point` (found while landing R1370, 2026-10-27).
  - Depends on nothing.
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

- [ ] **R1302 A bound heterogeneous collector is typed the tuple of its
  elements, where upstream's is the nominal `VariadicPack`**

  `*args: *Ts` binds `Tuple[...]` (`TyKind::Tuple`) in its specialized
  body, and `TyKind::VariadicPack` over the pack before it, where the pin
  types it `VariadicPack[elt_is_mutable, origin, Trait, *Ts]`, declared in
  `std/builtin/variadics.mojo` beside `VariadicList`.
  - Probe: `verify_pack_spread` admits a `Tuple`-typed spread argument,
    which every bound spread passes.
  - `mojito_types::types::variadic_collector_type` decides the body type;
    its heterogeneous arms are the ones to make nominal.
  - The homogeneous collector took upstream's `VariadicList` in R743.
  - Probe: `args.get_loaded_kgen_pack()`, which the bundled `llvm_intrinsic`
    calls, is a compiler rule typed as the collector itself
    (`mc_infer.rs`), where upstream's `VariadicPack` method loads its
    elements into a `!kgen.pack`.
  - Found while planning R743 (2026-10-23).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1315 A `!pop.array` size cannot be a parameter, so
  `_MLIR.POPArrayType` cannot be declared**

  Problem: MIR's `!pop.array` (`TyKind::PopArray`) holds a concrete size
  only, and the checker resolves no ``__mlir_type[`!pop.array<`, size, `,
  `, T, `>`]`` spelling, so the bundled `_MLIR` cannot declare upstream's
  `POPArrayType[size: __mlir_type.index, elt_type: AnyType]`.
  - Probe: adding upstream's `POPArrayType` member to `_MLIR`
    (`stdlib/std/builtin/variadics.mojo`) is rejected with "only ...
    '__mlir_type[`!kgen.param_list<`, T, `>`]' are accepted"
    (`checker/type_resolution.rs::mlir_applied_type`).
  - Add a `!pop.array<{},{}>` arm to `mlir_applied_type`. Its size slot is
    `Checker::slot_value` read by `integer_slot` (`type_resolution.rs`),
    which `!kgen.simd`'s length shares through `mlir_index_operand`; a
    `size: __mlir_type.index` binder is read bare, not through
    `_mlir_value`.
  - The size needs the `Known`/`Expr` shape `SimdWidth` has, substituted
    by the elaborator and refused symbolic by the concrete verifier.
  - Found while landing R1304 (2026-10-23).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1316 `!lit.ref` and `!kgen.pointer` values have no spelling**

  Problem: upstream's `VariadicList` constructor takes its array as a
  `Pointer[...]._mlir_lit_ref` (`!lit.ref<T, origin>`), turns it into a
  `Pointer` with `Pointer(_mlir_value=value)`, and reads its address with
  `_get_kgen_pointer()` (`!kgen.pointer<T>`), and Mojito accepts none of
  the three.
  - Probe: `def f[o: ImmOrigin](r: Pointer[Int, o]._mlir_lit_ref)` is
    rejected.
  - `Pointer` is a compiler built-in here (`TyKind::Pointer`), so each is a
    built-in member, as `unsafe_origin_cast` is
    (`checker/method_calls/builtin_types.rs`).
  - Found while landing R1304 (2026-10-23).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1317 `pop.array.gep` and the `index` conversions are not
  accepted**

  Problem: upstream's `VariadicList` constructor addresses its array's
  first element with ``__mlir_op.`pop.array.gep`(array_up._get_kgen_pointer(),
  Int(0).__mlir_index__())`` and reads its size parameter with
  `Int(mlir_value=size)`, and Mojito's `__mlir_op` admits only the two
  ownership markers (`checker/method_calls/mlir_op.rs`).
  - `Int(0).__mlir_index__()` and `Int(mlir_value=size)` are upstream's
    builtin casts, which run since R1516, and the size is a parameter of
    type `__mlir_type.index`, which binds a compile-time `index`.
  - The gep would be a `ref.make` of the array's element place, as the
    call's builder takes the span's pointer today
    (`mir::variadic_list::build_variadic_list`).
  - Found while landing R1304 (2026-10-23).
  - Depends on R1316 (the `!kgen.pointer` operand) and R221 (the
    constructor's `unsafe_bitcast` of the element pointer).
  - Model: Opus, Not Planned.

- [ ] **R1314 The call builds a `VariadicList`'s span itself, where
  upstream calls the list's `@implicit` constructor from the `!pop.array`**

  Problem: every call of a `*xs: T` callee stores its element pointers'
  `!pop.array` in `__passed_varargs__`, as upstream's does, then builds the
  `Span` and the list over it as two `struct.create`s
  (`mir::variadic_list::build_variadic_list`), where upstream calls
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
  - Depends on R1315, R1316, and R1317 (the constructor's vocabulary).
  - Model: Fable, Not Planned.

- [ ] **R1451 `remove-unused-params` still counts a read of a slot named
  after any value binder**

  Its parameter-use analysis (`slot_reads`, `mir/param_uses.rs`) counts
  any instruction naming a slot spelled like a value binder as a read of
  the binder, though since R662 only a callable binder or a pack takes a
  slot (`types::binder_read_takes_slot`); upstream's `RemoveUnusedParams`
  sees a read only as a use of the parameter attribute.
  - The pass cases under `tests/mir_passes/remove-unused-params/` still
    spell an `Int` binder's read as `var.use` of a slot named after it
    (`lifted.mir`, `forwarded.mir`, `default_kept.mir`,
    `forwarded_argument.mir`, `lifecycle_member.mir`), a shape MIR no
    longer lowers; narrowing `slot_reads` removes those binders.
  - Done when the cases read a binder as `const { value:
    param(param_decl_ref …) }`, as MIR lowers it today, and `slot_reads`
    keeps only the binders `binder_read_takes_slot` names and packs.
  - Found while landing R663 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1449 A callable value binder's read is a frame slot, not a
  parameter reference**

  A body reads a callable binder (`f` in `def apply[f: def(Int) thin ->
  Int]`) from a slot nothing stores, which the elaborator and
  `inline-parametric` rewrite by name into the bound function, where
  upstream reads it as `kgen.param.constant` of the binder like any other
  parameter.
  - The check keeps callable binders out of `Checker::value_binder_read`,
    though no instance now takes a bound closure as a runtime parameter
    (R533).
  - The by-name rewrites: `substitute_callable_binder_reads`
    (`mono/substitute.rs`) and `inline-parametric`'s `read_binder`.
  - `mem-2-reg` skips a slot nothing stores or declares for this reason
    (`variance`).
  - Probe: `f(x)` in `assets/ok/nested_def_reads_enclosing_parameter.mojo`
    and `assets/ok/thin_nested_def_reads_value_parameter.mojo`.
  - Probe: `F[4](x)` in `assets/ok/function_type_where_bound.mojo`.
  - Probe: the `f` slot `mem-2-reg` keeps in
    `assets/ok/unread_callable_parameter_instances.mojo`.
  - Probe: `callback` in
    `assets/ok/capturing_generic_callable_parameter_environment.mojo`.
  - A pack of callables (`*fs: def() thin -> None`) is the same slot, a
    `VariadicPack` nothing stores, kept out of the value scope, so it is
    no `ParameterList` the check reads.
  - Probe: `fs[i]()` in a `comptime for i in range(len(fs))` of `def
    run[*fs: def() thin -> None]` reports "'fs' has type
    $variadic[def() thin -> None] and is not callable"; the pin prints
    `a` and `b` over `run[a, b]()`.
  - Probe: `print(len(fs))` there reports "vm backend does not support
    methods on None yet"; the pin prints `2`.
  - Found while landing R662 (2026-10-26).
  - Depends on R533: a capturing closure must leave the runtime-parameter
    path first.
  - Model: Opus, Not Planned.

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
  - The leaf binders (`size_of[T]`, `_unqualified_type_name[T]`, the
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
  - `infer` re-runs `simd_dims` for a `SIMD` construction's dimensions
    after `infer_simd_construction` bound them (`inference.rs`).
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
  - A binding of an application already has no slot: `Checker::local_comptime_application` makes each read a `ParamValue`, and `CheckedStmtOp::Comptime::denotes` tells MIR.
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
    `max` and `min` from `std.math` for `clamp`.
  - Fix: bind the prelude's names in bootstrap modules too, then drop the
    six `range` imports and `simd.mojo`'s `max`/`min` import, and delete
    the `min`/`max` rule, its `call_inference.rs` arm, the VM's
    `"min"`/`"max"` `BUILTIN_CALLEES` entries and arms, and Pliron's
    `lower_min_max_builtin` with its `calls.rs` arm.
  - Found while landing R791 (2026-10-14).
  - Depends on R592 (the `max` callers only: `std.math` links after them).
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

- [ ] **R1540 A parameter list is indexed by an `Int`, where upstream's
  `__getitem_param__` takes a `SIMDLength`**

  `ParamContext::list_get` (`mojito-types/param_expr.rs`) takes only an
  `Int` index, where upstream's `TypeList.__getitem_param__[idx:
  SIMDLength]` indexes `Self.values` by a `SIMDLength`.
  - Probe: respelling `stdlib/std/builtin/variadics.mojo`'s
    `comptime __getitem_param__[idx: Int] = Self.values[idx]` as
    upstream's `idx: SIMDLength` fails to package the library with
    "parameter list element expects an Int parameter list index, found
    SIMDLength".
  - Lever: a `SIMDLength` index reads as its lane count
    (`types::simd_length_image`), as the SIMD width slot reads one; then
    respell `TypeList.__getitem_param__` as upstream's bracketed
    `#kgen.param_list.get` body over `idx._mlir_value` and drop its
    comment.
  - Probe: that body already runs over a user struct (`comptime
    get[idx: SIMDLength] = __mlir_attr[…, idx._mlir_value, …]`, read as
    `l.get[1]`); respelling the bundled member with it fails packaging
    with "not a compile-time Int constant: unsupported associated
    comptime member access".
  - `ParameterList.__getitem_param__` (R800) takes the same index.
  - Found while landing R1539 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R800 A compile-time subscript of a value pack is a checker rule, not
  `ParameterList.__getitem_param__`**

  `comptime e = vs[1]`, `Box[vs[0]]`, and `materialize[vs[i]]()` are
  compiled by the checker straight to a `ListGet` over the pack, where
  upstream reads them through `ParameterList.__getitem_param__`.
  - The checker site is the `Index` arm of `compile_dependent_ct_expr`
    (`checker/constraints.rs`), which also serves display lists and module
    display constants and must keep doing so.
  - Add `comptime __getitem_param__[idx: Int]: Self.type =
    Self.values[idx]` to `ParameterList` in
    `stdlib/std/builtin/variadics.mojo`, as `TypeList` already declares
    its own; once R1540 lands, both are upstream's bracketed
    `#kgen.param_list.get` body.
  - Upstream's index parameter is `idx: SIMDLength`, which a parameter
    list does not take as an index yet (R1540), so the port spells it
    `Int`, as `TypeList`'s does, and says so in the module header.
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
    ParameterList[Int, vs]"; the pin says 'ParameterList[vs.values]'.
  - Probe: R904's pack loop.
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
  - Probe: `d.__eq__(DType.int64)` over a `DType` fails with "type 'DType'
    has no method '__eq__'", where the pin prints `True`. The `==` and `!=`
    operators, and a leading-dot operand beside them, are a
    `TyKind::Dtype` arm of `infer_infix` that upstream's declared
    `DType.__eq__(self, rhs: Self)` replaces.
  - Port `DType.__hash__` with the struct and retire its hash-leaf arms
    (`builtin_hashable_ty`, `hash_leaf_ty`, the VM's `__hash__` arm,
    `lower_hash_leaf`, `enqueue_hash_leaf_instances`); R1192 retires the last leaf.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1608 `Bool` has no declaration, so its members are compiler
  rules and `Bool.MIN` is undefined**

  Problem: `print(Bool.MIN, Bool.MAX)` and `Bool.__eq__(True, False)`
  fail with "Undefined variable 'Bool'", where the pin prints `False True`
  and `False`, because there is no `struct Bool` under `stdlib/`.
  - Declare `struct Bool(Boolable, Defaultable, Floatable, Hashable,
    ImplicitlyCopyable, Intable, TrivialRegisterPassable)` in
    `std/builtin/bool.mojo` with upstream's field `var _mlir_value:
    __mlir_type.`!kgen.scalar<bool>``, `MIN`, `MAX`, the `__*_is_trivial`
    constants, and the bodies of `__bool__`, `__mlir_bool__`,
    `__mlir_i1__` (`pop.cast_to_builtin`), `__int__`, `__float__`, and
    `__hash__`.
  - Add the `("std.builtin.bool", "Bool")` row to
    `implicit_public_identity` (`mojito-module`), `PRELUDE_EXPORTS`, and
    the prelude import. Drop Python interop and
    `@lldb_formatter_wrapping_type`, as R1549 did for `SIMD`.
  - Bridge the declaration onto `TyKind::Bool` as R1549 bridges `SIMD`:
    `Ty::new`, `types::nominal_parts`, `declared_receiver_member`,
    `decide_type_conforms`, and the VM's `declared_owner`.
  - Delete the rules the bodies replace: `mc_infer.rs`'s `TyKind::Bool`
    early returns, `mlir_conversion`'s `__mlir_bool__`/`__mlir_i1__` arms,
    the VM `invoke.rs` and Pliron `methods.rs` conversion arms on a `Bool`,
    `modelled_conformance`'s `Bool` arms for the declared traits, and the
    `Bool` hash leaf (`builtin_hashable_ty`, both `hash_leaf_ty`s, the VM
    `__hash__` arm, `lower_hash_leaf`, `enqueue_hash_leaf_instances`).
  - A `Bool`'s `_mlir_value` stays the read `SemanticAdjustment::BoolStorage`
    while the bridge holds `Bool` as `i1`.
  - Probe: `hash(True)` and `hash(False)` stay `7121024052126637824
    14824966480498192933`.
  - Trap: check for an import cycle through `std.hash` → `std.simd` →
    `std.builtin.bool` (R592), and run the members natively, not only on
    the VM (R1549's `resolved.is_none()` arms).
  - Plan: `R1188-bool-is-a-compiler-modelled-scalar-plan.md`, entry A.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1609 `Bool`'s initializers are checker rules, so `Bool()` and
  `Bool(None)` are rejected**

  Problem: `print(Bool(), Bool(None))` fails with "expected a numeric or
  Bool value, found None", where the pin prints `False False`, because
  `Bool(...)` is typed by `infer_unbound_call`'s and `builtins.rs`'s
  conversion rules rather than upstream's overloads.
  - Declare the six non-Python overloads: `__init__(out self)`,
    `@implicit __init__(value: __mlir_type.i1)` over
    `pop.cast_from_builtin`, `@implicit __init__(mlir_value: …)`,
    `__init__[T: Boolable, //](value: T)`, `__init__(value: None)`, and
    `@implicit __init__(value: Scalar[DType.bool])`.
  - Construction is declared-first with the rule fallback, as R1555 did
    for `SIMD`.
  - `self._mlir_value = …` in an initializer lowers as a `scalar.cast` to
    `Bool` into `self`'s place, the store half of `BoolStorage`, which the
    checker rejects outside an initializer today.
  - Delete the two `"Bool"` arms of `infer_unbound_call` and the `Bool`
    arm of the conversion rule in `builtins.rs`.
  - The `@implicit` stand-ins in `Checker::record_implicit_conversion`
    (`i1` and `Scalar[DType.bool]` to `Bool`) stay until R1612.
  - Probe: `Bool(Foo(3))` over a user `Boolable`, `Bool(3)`, `Bool(0.0)`,
    and `Bool(value=t)` still print as at the pin.
  - Depends on R1608.
  - Model: Opus, Not Planned.

- [ ] **R1610 `Bool`'s comparison and bitwise operators are checker rules,
  so `t.__eq__(f)` has no method**

  Problem: `t.__eq__(f)` and `t.__invert__()` over `Bool`s fail with "type
  'Bool' has no method '__eq__'", where the pin prints `False` and the
  inverse, because `==` and `~` over a `Bool` are `infer_infix` and
  `infer_prefix` arms (`checker/operators.rs`).
  - Declare upstream's `__eq__`, `__ne__`, `__lt__`, `__le__`, `__gt__`,
    `__ge__`, `__invert__` (over a typed
    ``__mlir_attr.`#kgen.simd<true> : !kgen.scalar<bool>` ``), the
    `and`/`or`/`xor` dunders with their `__i*__` and `__r*__` forms, and
    `Comparable`.
  - Delete the `TyKind::Bool` arms of `infer_prefix` and `infer_infix`.
    `not` stays a rule, since upstream's is not a dunder.
  - `comptime x = True == False` and `~True` must keep folding: follow
    whichever of R1551/R1553's routes kept `SIMD`'s operators folding.
  - Probe on the pin whether `True == 1` and `b == mask` take an implicit
    conversion or the reflected dunder.
  - Depends on R1608.
  - Model: Opus, Not Planned.

- [ ] **R1611 `Bool.write_to` and `write_repr_to` are compiler printing
  where upstream's are library bodies**

  Problem: `Bool`'s `Writable` is a modelled conformance, because
  upstream's body writes `StaticString("True") if self else
  StaticString("False")` and `StaticString` is undefined (R432).
  - Declare both members and `Writable`, and delete `Writable`'s `Bool`
    arm in `modelled_conformance`.
  - `print(b)`'s builtin path stays only while `SIMD`'s does (R276).
  - Depends on R1608 and R432.
  - Model: Opus, Not Planned.

- [ ] **R1612 `Bool` is `TyKind::Bool` in the checker, where upstream's is
  the declared struct**

  Problem: once R1608–R1611 land, the `Ty::new` bridge and the `@implicit`
  stand-ins are all that keep `Bool` a compiler type.
  - Delete the bridge, so `Bool` resolves to `Struct("Bool", [])`, and
    `nominal_parts`'s `Bool` arm.
  - Delete the `i1` and `Scalar[DType.bool]` to `Bool` conversions in
    `Checker::record_implicit_conversion` for the declared `@implicit`
    overloads, and `ScalarConversion::casts_from`'s `i1` arm.
  - Decide whether MIR keeps holding the struct as an `i1` (R1396's
    one-field register-passable hold), and with it `types::held_ty`'s
    `i1`-as-`Bool` arm, `require_bool`'s `i1` arm, and the VM's
    `builtin.cast` conversion between `Value::Bool` and a one-lane mask.
  - Like R1557 for `SIMD`.
  - Depends on R1608, R1609, R1610, R1611, and R1189.
  - Model: Fable, Not Planned.

- [ ] **R1189 `Bool` does not conform to `EnumLike`**

  Problem: `count[Bool]()` over `def count[T: EnumLike]()` is rejected,
  where the pin's `Bool` conforms with cases `False` and `True` and no
  payloads.
  - Give the ported struct upstream's members (`bool.mojo`): the names
    and payload types by `ParameterList.of` and `TypeList.of`,
    `_get_enum_discriminant` returning `self.__int__()`, and the
    never-called payload getter.
  - Probe: the `Bool` half of
    `conformance/probes/enum_like_optional_bool.mojo`.
  - Delete the modelled-`Bool` arm of `enum_constructors`
    (`checker/coverage.rs`) once the library `Bool` conforms.
  - Found while planning R1132 (2026-10-20).
  - Depends on R1132 (the trait) and R1608 (a library `Bool`).
  - Model: Opus, Not Planned.

- [ ] **R1192 `StringLiteral` is a compiler-modelled type where upstream's
  is `StringLiteral[value: !kgen.string]`**

  Problem: `print(StringLiteral["None".value]())` fails with "Undefined
  variable 'StringLiteral'", where the pin prints `None`, because Mojito's
  string literal type has no `value` parameter.
  - `StringLiteral` is `TyKind::StringLiteral`, and a literal's `.value` is
    a compiler fold (`CtValue::literal_value`) rather than a read of the
    type's parameter.
  - Port upstream's `std/builtin/string_literal.mojo` struct, the way R1608
    ports `Bool`, so a literal's type is the application its text keys.
  - Probe: `StringLiteral[v]().byte_length()` over `comptime v =
    "None".value`, which prints `4` at the pin.
  - Converting a KGEN string to a runtime string (`StaticString(names[i])`)
    then follows with R432: upstream's `StringSpan.__init__(out self:
    StaticString, _kgen: !kgen.string)` over `pop.string.size` and
    `pop.string.address`.
  - Probe: `get_static_string["ab", "cd"]()`, which upstream declares in
    `std.collections.string.string_span` as
    `StaticString(_get_kgen_string[string, *extra]())` and the bundled
    module omits.
  - Probe: `"x" + s` and `"ab" == s` over a `String` `s` convert the
    literal to `String` and call `String`'s dunder (`infer_infix`'s
    literal-left arm, `checker/operators.rs`), where upstream calls
    `StringLiteral.__eq__(self, rhs: StringSlice)`, and for `+` the
    reflected `String.__radd__(self, other: StringSlice)`, which the
    bundled `stdlib/std/string.mojo` lacks. The port deletes that arm.
  - Port `StringLiteral.__hash__` with the struct and retire its hash-leaf
    arms (`builtin_hashable_ty`, the VM's `__hash__` arm,
    `lower_hash_leaf`, `enqueue_hash_leaf_instances`).
  - Once R160 and R1608 have retired the `DType` and `Bool` leaves too,
    `__hash__` and `_update_with_simd` leave `mir::TYPE_REACHED_MEMBERS`,
    and Pliron's type-reached member push (`lib.rs`) goes with them.
  - The planning session splits it into chained one-session entries if
    one session cannot land it.
  - Found while landing R1183 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1520 `!kgen.simd<n, address>` has no pointer lane**

  ``__mlir_type[`!kgen.simd<`, size._mlir_value, `, address>`]`` is
  rejected by `mlir_applied_type`, where the pin types a vector of pointers.
  - `TyKind::Simd` lanes are `Dtype`s, and no `Dtype` is an address.
  - Probe: upstream's `masked_gather` and `masked_scatter`
    (`std/sys/intrinsics.mojo`) pass their base pointers in that type to
    `llvm.masked.gather`/`scatter`.
  - Found while planning R1394 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1521 An LLVM intrinsic outside Mojito's table is rejected, where
  the pin calls it**

  A reached `llvm_intrinsic` call of any intrinsic the table does not list
  fails elaboration ("the LLVM intrinsic '…' is outside the subset"), where
  the pin calls any intrinsic LLVM knows.
  - The VM computes each intrinsic itself (`runtime::llvm_intrinsic`), so
    each one needs a lane rule in `mojito_types::llvm_intrinsic` before
    either backend admits it.
  - Probe: `llvm_intrinsic["llvm.sin", Float64, has_side_effect=False](x)`
    and the rest of `math`'s `_llvm_unary_fn` family (`cos`, `asin`,
    `acos`), `llvm.umulh`/`smulh`, `llvm.usub.sat`, and `llvm.is.fpclass`.
  - Probe: `std.sys.intrinsics`' `assume`, `expect` (`likely`/`unlikely`),
    `prefetch`, and the `llvm.memcpy`/`memmove` calls of `std.memory`.
  - Target- and GPU-specific intrinsics (`llvm.x86.*`, `llvm.nvvm.*`,
    `llvm.amdgcn.*`) stay outside: the VM has no target to read.
  - Found while landing R1394 (2026-10-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1524 A one-field `RegisterPassable` struct with lifecycle
  members is a boxed struct value in the VM**

  The pin holds `R(RegisterPassable){v: Int}` with a `__del__` as its
  element (`define internal i64 @"u3::fr(u3::R$)"(i64)` in unoptimized
  LLVM), as it holds a trivial one, where the VM keeps it a `Value::Struct`.
  - Cause: `drop_value`, `clone_value`, and `move_typed_value`
    (`backend/vm/dispatch.rs`, `values.rs`) find `__deinit__`,
    `__copyinit__`, and `__moveinit__` by the value's runtime struct name,
    which a value held as its field no longer carries.
  - `MirDeclarations::register_field` therefore answers only a struct
    declaring `TrivialRegisterPassable` unconditionally, which owns none.
  - Lever: every drop, copy, and move dispatches by its static type, as
    `drop.var` and `drop.place` already name their destructor. Then
    `register_field` widens to `register_passable` at the instance, which
    holds `VariadicList` and a register-passable `Tuple` instance too.
  - The VM's `build_structs` keeps every instance of a template held or
    none, so a per-instance answer stays consistent with an erased
    instance.
  - Found while landing R1396 (2026-10-27).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1562 `CompilationTarget` is undeclared, where upstream's
  `std/sys/info.mojo` answers target queries from `#kgen.param.expr`
  attributes**

  `from std.sys import CompilationTarget` finds no declaration, where the
  pin runs `CompilationTarget.has_avx2()` and `simd_bit_width()`.
  - Mojito has no `!kgen.target` type, and folds none of the
    `current_target`, `target_get_field`, or `target_has_feature`
    attributes upstream's bodies read.
  - Add `!kgen.target` as a compile-time-only type with a
    `#kgen.target<…>` constant (upstream's `_empty_target`), listed in
    `types::has_no_llvm_type` so the VM holds it as an attribute value and
    translation legality rejects one that survives, as for
    `!kgen.deferred`.
  - Keep `current_target` a symbolic application through the check, as the
    pin keeps `_current_target()` an `apply` expression. Mono answers it
    from the elaborator's `NativeTarget`, beside the layout oracle's
    `size_of`, so one checked program serves any `--target`.
  - Fold `target_get_field` and `target_has_feature` over a closed target.
  - At the pin `has_feature(name)` is literal `+name` membership of the
    target's feature list (`_has_feature["bogus"]` is `True` under
    `--target-features +bogus`), and `TargetCpu::has_feature` answers it.
  - `simd_bit_width` is 512 under `+avx512f`, else 256 under `+avx2`,
    else 128 (AVX without AVX2 is 128).
  - Port `_TargetType`, `_current_target`, `_empty_target`, the
    `CompilationTarget` struct with `current` and `_unknown`,
    `_has_feature`, the `has_*` x86 queries, and `simd_bit_width` and
    `simd_byte_width`, verbatim.
  - Fixture: `_has_feature["sse2"]()`, the `_unknown` answers (`False`,
    `0`), and a `comptime if` decided by `has_avx512f()`.
  - The `size_of`/`align_of` bodies are R1634, and the string predicates
    are R1635.
  - The Pliron test helpers pin the backend to the `x86-64` baseline while
    their `Compiler::default()` elaborates for the host CPU. Once target
    queries fold, hand the helpers' pinned target to
    `Compiler::with_target` too, so elaboration and lowering agree.
  - Depends on R1536 (every query body is a bracketed `__mlir_attr`),
    R1630 (`_current_target()` returns `!kgen.target`), R1631
    (`CompilationTarget.has_avx2()` is a bare-name static call), R1632
    (`comptime current = CompilationTarget[…]` names its own struct), R1640
    (the answers are the host CPU's), and R1641 (`_has_feature` takes a
    compile-time `StaticString`).
  - Model: Fable, Planned.

- [ ] **R1634 `size_of` is a compiler name and `align_of` is missing,
  where upstream's are library bodies over `get_sizeof` and `get_alignof`**

  `size_of[DType.int32]()` is rejected ("expected a type, found a value")
  and `from std.sys.info import align_of` finds nothing, where the pin
  prints `4` and `align_of[Float64]()` prints `8`.
  - The checker serves `size_of` by name: `call_inference.rs` records
    `SemanticAdjustment::SizeOf` (`MirInstr::SizeOf`), `constraints.rs`
    builds `SIZE_OF_FUNCTION`, and `simd_width.rs` reads it, and module.rs's
    `COMPILER_NAMES` lists it.
  - Port both overloads of `size_of` and `align_of` with their `target`
    parameter, as upstream's `#kgen.param.expr<get_sizeof|get_alignof, …>`
    bodies. Mono's `layout_answer` already answers the size.
  - Restore `target: CompilationTarget = CompilationTarget.current()` on
    `bit_width_of` (dropped in R1550), and port `simd_width_of`,
    `is_32bit`, and `is_64bit`.
  - Retire `SemanticAdjustment::SizeOf` and `MirInstr::SizeOf` (VM, Pliron,
    MIR text with a schema bump, verify, cse, the specializer).
  - `size_of_builtin`, `comptime_if_layout_query`, and
    `comptime_layout_constant` keep their output.
  - Depends on R1562 (`CompilationTarget`) and R1606 (the `target`
    parameter's bare `CompilationTarget` annotation).
  - Model: Fable, Planned.

- [ ] **R1635 `CompilationTarget`'s string-field predicates are
  undeclared**

  `CompilationTarget.is_x86()`, `is_linux()`, and `_os()` are undefined,
  where the pin prints `True`, `True`, and `linux` on this host.
  - Each body reads a `!kgen.string` field through `target_get_field` and
    compares it as `StringLiteral[Self.__triple_arch()]()._is_identical(…)`,
    so it needs `StringLiteral`'s `value` parameter (R1192) and a
    compile-time `StaticString` (R1641).
  - Port `is_triple`, `_arch`, `__arch`, `_is_arch`, `__triple_arch`,
    `__triple`, `_is_triple_arch`, `_eq_triple_and_arch`, `_os`, `is_x86`,
    `is_arm`, the RISC-V, Apple, and Neon queries, `is_linux`, `is_macos`,
    `is_cpu`, `is_accelerator`, `is_gpu`, `default_compile_options`, and
    `unsupported_target_error`.
  - Port the free functions over them too: `is_gpu`, `is_nvidia_gpu`,
    `is_amd_gpu`, `is_apple_gpu` and their `_is_sm_*` and `_is_amd_*`
    families, `is_little_endian`, `is_big_endian`, `Endian`, and
    `platform_map`.
  - The GPU predicates answer `False` on a CPU target, so `bitcast`'s and
    `SIMD.cast`'s GPU guards (`std/memory/unsafe.mojo`, `std/simd.mojo`)
    can be restored.
  - `_arch()` is `StaticString(Self.__arch())`, a run-time call of a
    function returning `!kgen.string`, which needs R1190's run-time form.
  - Depends on R1562, R1192, R1641, and R1190.
  - Model: Opus, Planned.

- [ ] **R1623 `SIMD`'s `%`, `divmod`, and `ceildiv` are checker rules
  and a trap, where upstream's are `__mod__`, `__divmod__`, and
  `__ceildiv__` bodies**

  `Int(7) % 0` traps here, where upstream's `__mod__` masks a zero divisor
  and returns `0`, and `Float32(7.5) % 2.0` is rejected (R850).
  - The signed branches of `__mod__` and `__divmod__` compute `self /
    safe_divisor` over integer lanes, which is upstream's truncating
    `__truediv__` (R184). Today's `/` gives `3.5` or garbage there, so the
    port waits on R184 and must not respell `/` as a raw `pop.div`.
  - Port `__mod__`, `__divmod__`, `__rmod__`, `__imod__` (`simd.mojo:1735`),
    and `__ceildiv__` verbatim, beside the landed `__floordiv__`, and route
    `%` in `Checker::infer_infix` as `//` is routed.
  - `std.simd` imports `CeilDivable`, `DivModable`, and `trunc` from
    `std.math`, and `any` from `std.builtin.bool`.
  - `__ceildiv__`'s unsigned branch calls `divmod(self, denominator)`:
    `Checker::infer_divmod` must type a `SIMD` pair, a symbolic one
    included, as `Tuple[common, common]`, and the elaborator's builtin
    rewrite (`mono/specializer.rs`) must make it `__divmod__` for every
    `TyKind::Simd` operand.
  - List `CeilDivable` and `DivModable` in `SIMD`'s declaration, and drop
    their `modelled_conformance` arms.
  - Retire the VM's `__ceildiv__`/`__divmod__` arms
    (`backend/vm/invoke.rs`), Pliron's (`lower/methods.rs`,
    `lower_ceildiv`), the checker's `math_operation_requirement` rows, and
    `simd_infix`'s `Mod` arm; `builtin_fold.rs` reads `__mod__`'s inline
    level as it reads `__floordiv__`'s.
  - `assets/runtime_error/pliron_trap_mod_zero.mojo` then runs: move it to
    `assets/ok` with a `print(x)`, drop its ledger and manifest rows, and
    update `docs/non-goals.md`'s trap sentence.
  - Add `assets/type_error/simd_bool_mask_inplace_mod.mojo` (the pin:
    "the SIMD type must be numeric", `simd.mojo:1744`).
  - Probe: `print(Int(7) % 0, divmod(Int(7), 0), ceildiv(UInt8(250),
    UInt8(7)), Int(-7) % 2, Float64(-7.5) % 2.0)` prints `0 (0, 0) 36 1
    0.5` at the pin; a 2026-10-29 prototype of this port matched it on
    both backends once `/` truncated.
  - Probe: `comptime r = UInt(7) % 0`, and `comptime k = n % 0` inside
    `def f[n: Int]`, trap in their compile-time run, where the pin's
    `__mod__` makes each `0`; `W[n % 0]` in a parameter position already
    folds to `0`.
  - Closes R850.
  - Depends on R184.
  - Model: Fable, Planned.

- [ ] **R1567 `SIMD`'s `**` is a checker rule, where upstream's is
  `__pow__` over `_pow`**

  `v ** 2` over a `float32` vector is rejected, and `Float64(2) **
  Float64(0.5)` prints the host's `powf`, where the pin prints
  `1.4142135623734946` through `_powf_scalar`'s `exp(log)`.
  - Port the three `__pow__` overloads, `__rpow__`, `_pow`, `_powf`,
    `_powf_scalar`, and `_powi`.
  - `_pow` reads `CurrentPlugin.binary_math_fn` (R330, R1562), stores
    lanes with `result[i] = …` (R1561), and initializes `out result` with
    `result = {}`.
  - Port `__ipow__` (`simd.mojo:1748`) too; its `rhs` is `Int`.
  - Retire the `__pow__` arms of `math_operation_requirement` and the
    elaborator's `__pow__` rewrite (`mono/specializer.rs`).
  - Removes `pliron_float_pow.mojo` from R171.
  - List `Powable` in `SIMD`'s declaration, and drop its
    `modelled_conformance` arm for `SIMD`.
  - Depends on R1566, R1561, R1562, R330, and R1568.
  - Model: Fable, Planned.

- [ ] **R1600 A `pop` op's fast-math flags are checked and dropped, where
  upstream evaluates them and carries them to LLVM**

  `fastmathFlags=` is typed as a compile-time `!kgen.deferred` value and
  then dropped before MIR, so no flag reaches either backend.
  - A computed flag such as `flag._mlir_attr()` now runs at compile time
    (R1630), but the flag it yields is still dropped.
  - `MirInstr::Pop` and the float `binary` ops carry no flags, and
    Pliron passes `FastmathFlagsAttr::default()` everywhere
    (`lower/simd.rs`).
  - Probe: `var a = __mlir_attr.`#pop.fmf<none>`` in `main` is rejected
    ("read at run time is not supported"), where the pin elides it. That
    rejection is R1638's.
  - Exact arithmetic implements every flag (`mlir::is_fastmath_attribute`),
    so output never differs: this is shape, not semantics.
  - Found while landing R1580 (2026-10-28).
  - Depends on R1630, which runs a function returning a flag at compile
    time.
  - Model: Fable, Not Planned.

- [ ] **R1582 `SIMD.__round__` is a builtin rule and `SIMD` does not
  declare `Roundable`**

  `round(v)` over a `SIMD[DType.float32, 4]` is rejected ("does not
  conform to trait"), where the pin prints the rounded lanes, and
  `resolve_builtin_method` serves `__round__` on `Int`, `UInt`, and
  `Float64` by name.
  - Port both overloads together: once the declaration has a
    `__round__`, every `__round__` call reads it, so a lone `__round__()`
    breaks `Float64(x).__round__(2)`.
  - The `ndigits` overload computes `Self(10) ** ndigits` (R1567).
  - List `Roundable`, and drop its `modelled_conformance` arm for
    `SIMD`.
  - Depends on R1567.
  - Model: Fable, Planned.

- [ ] **R1585 `SIMD`'s reductions are checker rules, where upstream's are
  a `reduce` tree**

  `infer_simd_method`'s `reduce_*` arm, `runtime::simd_method`, Pliron's
  `lower_simd_reduce`, and `INTRINSIC_METHODS` serve `reduce_add`,
  `reduce_mul`, `reduce_max`, and `reduce_min` by name.
  - Port both `reduce` overloads (`simd.mojo:3107`, `:3135`), then
    `reduce_add`, `reduce_mul`, `reduce_max`, and `reduce_min`.
  - Their bodies read `comptime _T = SIMD[Self.dtype, _]` as `Self._T[n]`,
    which runs, apply a callable binder at a `SIMDLength` (R1583), spell the
    closure overload's binder as a trait composition with a function type
    (R1584), and call `split()` (R330).
  - `reduce_max` and `reduce_min` take `llvm.vector.reduce.*` only off
    x86 (`CompilationTarget.is_x86()`, R1635); the host is x86_64, so the
    tree runs.
  - Closes R187: `reduce_mul` over the sixteen `float32` lanes `1.1, 2.3,
    3.7, 4.9, 5.3, 6.1, 7.7, 8.3, 9.9, 10.1, 11.3, 12.7, 13.9, 14.1, 15.3,
    16.7` prints `68104130000000.0` at the pin and `68104122007552.0`
    here.
  - Depends on R296, R1583, R1584, R330, and R1635.
  - Model: Fable, Planned.

- [ ] **R1586 `reduce_and` and `reduce_or` are checker rules and
  `reduce_bit_count` is missing**

  `infer_simd_method` types `reduce_and` and `reduce_or` as `Bool` through
  `SemanticAdjustment::SimdReceiverOp`, where upstream's return
  `SIMD[Self.dtype, size_out]`.
  - Port both and `reduce_bit_count`, with `pack_bits` (R1578) and
    `std.bit.pop_count`.
  - Each body holds `self.reduce[size_out](…)` under `comptime if
    size_out > 1`, which a template checks too (R1585).
  - The result type needs `and`/`or` over a lane (R97); `__contains__`'s
    `-> Bool` return then converts it.
  - Retire `SemanticAdjustment::SimdReceiverOp` and
    `record_simd_receiver_op`.
  - Depends on R1585, R1578, and R97.
  - Model: Fable, Planned.

- [ ] **R1591 `SIMD`'s `mlir_value=` initializer is a checker rule that
  binds from a concrete storage type**

  `SIMD(mlir_value=v._mlir_value)` over a concrete `v: SIMD[DType.int32,
  4]` prints the vector in Mojito, where the pin rejects it ("no matching
  function in initialization"). An annotated `var`, a `return`, and an
  argument position are rejected at the pin too.
  - Upstream's `__init__(out self, *, mlir_value: Self._mlir_type)` binds
    `dtype` and `length` only where the argument's storage type spells both
    through binders (`!kgen.simd<width._mlir_value, dtype._mlir_value>` in a
    body generic over them, as upstream's `cast` and `select` build it).
    A concrete slot binds nothing, and neither does a literal width inside
    a generic body (`SIMD[dtype, 2]._mlir_type`).
  - Mojito's stand-ins: `lone_storage_keyword` and
    `infer_inferred_storage_construction`, the `SIMD[dt, n](mlir_value=v)`
    arm of `infer_unbound_call`, and `builtin_copy_constructed` for
    `Self(mlir_value=…)` (`checker/call_inference.rs`).
  - Declaring the overload today breaks the inlined `cast` body's
    per-instance re-check: its call names `SIMD.__init__$ov$$$KgenSimd…`
    while the declaration is `…$u5F$mlir$u5F$type…` (R240).
  - Then a `!kgen.simd` parameter pattern unifies a slot only with a
    binder-valued slot, the stand-ins retire, and the concrete-`v` program
    becomes a `type_error` fixture.
  - Keep the template's selection in a per-instance re-check, where the
    slots are concrete.
  - Found while landing R1555 (2026-10-28).
  - Depends on R240.
  - Model: Fable, Planned.

- [ ] **R1592 `SIMD`'s element-list initializer is a checker rule**

  `check_simd_args`, `TypeError::SimdArity`, and `try_simd_call` type and
  build `SIMD[dt, n](a, b, …)`, where upstream declares `__init__(out self,
  *elems: Scalar[Self.dtype], __list_literal__: NoneType = None)`
  (`simd.mojo:941`).
  - Its `lit.ownership.mark_initialized` over `self` is rejected:
    `std/simd.mojo` is not in
    `overload_support::is_bundled_private_storage_source`, and
    `origin_place` refuses a bridged `SIMD`'s `out self` even once it is.
  - Its `assert Self.length == len(elems)` is off by default at the pin
    (R1598). Omit it with a header note. `SIMD[DType.int32, 4](1, 2)` then
    compiles and traps at run time, as the pin's crashes: move its fixture
    to `runtime_error` and delete `SimdArity`.
  - Probe: `SIMD[DType.int, 2](m, m)` over `m = SIMDLength(3)` prints `[3,
    3]` at the pin, each element converting through the declared
    `SIMDLength` splat. Mojito's `splats_to` has no `SIMDLength` arm.
  - Remove the element list from `Checker::binds_undeclared_overload`.
  - Found while landing R1555 (2026-10-28).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1593 `SIMD` has no `from_bits=` initializer**

  `Float32(from_bits=UInt32(1065353216))` prints `1.0` and
  `SIMD[DType.bool, 2](from_bits=SIMD[DType.uint8, 2](0, 3))` prints
  `[False, True]` at the pin. Mojito rejects both ("keyword arguments are
  not supported here").
  - Upstream declares `@staticmethod def __init__[int_dtype: DType,
    //](out self, *, from_bits: SIMD[int_dtype, Self.length])`
    (`simd.mojo:991`), over the bundled `bitcast` and `ne(0)._refine`.
  - The parser reads a `@staticmethod`'s `out self` as an unannotated
    parameter, user structs included: `@staticmethod def __init__[T:
    Intable, //](out self, *, from_t: T)` is "Parameters require a type
    annotation". Grammar first, then `parse_method` (`parser/items.rs`).
  - Found while landing R1555 (2026-10-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1596 `SIMD` has no `MAX`, `MIN`, `MAX_FINITE`, or `MIN_FINITE`**

  `SIMD[DType.int32, 1].MAX` is "a parameterized type is not a value",
  where the pin prints `2147483647`, and `Float32.MAX` prints `inf`.
  - Port the four `comptime` members (`simd.mojo:638`) over upstream's
    `std/utils/numerics.mojo` `inf`, `neg_inf`, `max_finite`,
    `min_finite`, `max_or_inf`, and `min_or_neg_inf`, imported as
    `_max_or_inf` and its siblings.
  - `inf` and `neg_inf` need typed float attribute constants
    (``#kgen.simd<"inf"> : !kgen.scalar<f32>``).
  - `max_finite` needs `~Scalar[_unsigned_integral_type_of[dtype]()](0)
    >> 1`.
  - `Int32.MAX` also needs R66's scalar alias read as a type.
  - Probe: `type_of(v).MAX` over a `Float32` `v` reads the member on the
    type and reports "type 'Float32' has no field 'MAX'".
  - Found while landing R1555 (2026-10-28).
  - Depends on R66.
  - Model: Fable, Not Planned.

- [ ] **R1597 A compile-time `SIMD` construction folds by the checker's
  rule**

  `comptime v = SIMD[DType.int32, 4](1, 2, 3, 4)` is folded by
  `simd_construction` (`checker/constraints.rs`), and
  `comptime_requests.rs` excludes `SIMD` from requests. The pin folds a
  `builtin` `__init__` body (`foldInlineBuiltinFunction`) and interprets
  the variadic one.
  - The declared default initializer's `comptime res = SIMD[Self.dtype,
    Self.length](0)` folds through the same rule.
  - Fold `pop.simd.splat` into a parameter expression (`builtin_fold.rs`
    declines every typed op), and request the variadic one as any
    non-builtin compile-time call.
  - The values agree today: the gap is structural.
  - A SIMD instance's constructor is no application: `Checker::selected_call`
    declines one whose body does not fold, since `Specializer`
    (`native/mono/specializer.rs`) binds only a `TyKind::Struct` instance
    ("reads an instance that is no struct `SIMD[DType.bool, 4]`"), and past
    `nominal_parts` the VM's CTFE fails "expects 2 argument(s), got 1".
    Delete that gate once the `builtin` constructors fold.
  - So a local `comptime M = SIMD[DType.bool, 4](fill=True)` or `comptime
    Y = SIMD[DType.int32, 2](from_int=5)` lifts a thunk and prints as at
    the pin, but a position with no thunk denotes nothing.
  - Probe: `f[SIMD[DType.bool, 2](fill=True)]()` over `def f[m:
    SIMD[DType.bool, 2]]()` is "not an associated comptime expression";
    the pin prints `[True, True]`.
  - Probe: `SIMD[DType.int32, inf(SIMD[DType.int32, n](0))]` over `def
    inf[n: Int, //](x: SIMD[DType.int32, n]) -> Int` fails the same way;
    the pin prints `[1, 1]` for `n = 2`.
  - Found while landing R1555 (2026-10-28).
  - Depends on R1592.
  - Model: Fable, Not Planned.

- [ ] **R1574 A compile-time call of `SIMD`'s comparison members runs a
  lifted thunk, where upstream folds their `pop.cmp` bodies**

  `comptime m = v.lt(w)`, `v.__eq__(v)`, and `Bool(v)` over a parameter
  `v` are lifted into a `$comptime$` function the VM runs, where upstream's
  `foldInlineBuiltinFunction` folds the `@always_inline("builtin")` body.
  - `builtin_fold.rs` declines `pop.cmp` and `pop.simd.reduce_and`/
    `reduce_or`, so `eq` … `le`, `__eq__`, `__ne__`, and `__bool__` have no
    template.
  - Lever: elementwise `ParamOp`s for `pop.cmp` (predicates normalized to
    `eq`/`ne`/`lt`/`le` by swapping operands) and for the two reductions,
    built in `scalar_storage_expression` (`constraints.rs`) beside
    `mlir_op_operator`, with `Bool(mlir_value=…)` read as the value.
  - Canonicalize so the dunders collapse to today's nodes:
    `reduce_and(cmp_eq(a, b))` is `Eq(a, b)`, `reduce_or(cmp_lt(a, b))` is
    `Lt(a, b)`, `cmp_ne(m, splat(False))` over a mask is `m`, and a
    one-lane reduction is the identity, so `where n < 3` keeps its form.
  - The new `ParamOp` spellings bump the MIR minor and the package format.
  - Found while landing R1553 (2026-10-28).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1556 Code outside `mojito-types` matches `TyKind::Simd` and
  `TyKind::INT` by pattern**

  167 `TyKind::Simd` and 115 `TyKind::INT`/`UINT`/`FLOAT64` pattern sites
  span the checker, MIR, the elaborator, Pliron, and the VM, so R1557 and
  R1558 would each touch all of them.
  - Replace them with `types::simd_slots`, `Ty::is_int()`, and their
    siblings. No behavior changes.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1557 `SIMD` and its aliases are compiler types in the checker,
  where upstream's are the declared struct and `comptime` aliases**

  Once every member is a library body, the `Ty::new` bridge and
  `type_resolution.rs`'s `"SIMD"`/`"Scalar"` name rules are all that keep
  `SIMD` a compiler type.
  - Delete the bridge: `SIMD[...]` resolves to `TyKind::Struct("SIMD",
    …)` through the declaration.
  - `Scalar`, `Int`, `UInt`, `Float64`, `Byte`, and the sized aliases come
    from `std/simd.mojo`'s `comptime` aliases, in the pin's prelude list.
  - Delete what the bridge needed beside it: `self_receiver_ty`'s `SIMD`
    arm (`mojito-mir`), and `types::nominal_parts`'s `Simd` arm, which the
    elaborator, the checker, and the VM's `declared_owner` read `SIMD`'s
    identity through.
  - Delete the rule fallback of `SIMD` construction once the initializer
    entries empty it: `Checker::{declared_simd_construction,
    binds_undeclared_overload, simd_alias_instance}` and the rule arms of
    `infer_unbound_call` (`checker/call_inference.rs`).
  - The checker's implicit conversions into `SIMD` (`coerces`, `splats_to`)
    stand in for the `@implicit` overloads until the bridge goes.
  - Depends on R1555, R276, R330, R1556, R1591, R1592, R1593, R1595,
    R1399, and R1507.
  - Model: Fable, Planned.

- [ ] **R1558 MIR and the backends hold a `SIMD` value as a vector type,
  where upstream holds the struct as its `!kgen.simd` field**

  `TyKind::Simd` stays below the checker after R1557, and the MIR verifier
  compares it with `TyKind::KgenSimd` as one held representation
  (`verify/types.rs`, `types::held_simd_ty`).
  - Delete the nominal `TyKind::Simd`. `KgenSimd` is the only vector type.
  - A `SIMD` register is a held one-field struct (R1396's rule,
    `MirDeclarations::translated_type` for Pliron).
  - Delete the verifier's equivalence and the `held_simd_*` helpers.
  - Depends on R1557.
  - Model: Fable, Planned.

- [ ] **R1618 A compile-time argument computed by an inlined dunder keeps
  its function's parameter**

  `subscript_expr_fwd[n]`, returning `Grid(7)[n + 1]` over a
  `__getitem_param__[i]` that never reads `i`, keeps `n` and makes one
  instance per `n`, where `Grid(5)[10 // n]` removes it.
  - Since R1551 `n + 1` is `SIMD.__add__`'s inlined body, a region whose
    result is the argument, where `10 // n` is still a `binary` the
    forward analysis (`mir::param_uses`) traces into the removed binder.
  - Upstream folds the `@always_inline("builtin")` dunder into the
    parameter expression `n + 1`, which forwards as any other.
  - `forwarded_parameters_leave_the_instance_key` in
    `tests/compiler_test.rs` expects two instances until this lands.
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

- [ ] **R1622 Twenty-one `inline-parametric` pass cases spell a value
  binder read as a frame-slot read, which lowering no longer emits**

  Since R662 a read of a non-callable value binder lowers to its parameter
  constant (`const { value: param(param_decl_ref { … }) }`), and the inliner
  no longer rewrites a binder's slot. The cases under
  `tests/mir_passes/inline-parametric/` written before that still read the
  binder through a `var.use` of a slot nothing stores, so the splice now
  leaves those reads in place and 21 expectations fail.
  - Failing cases include `generic_value`, `generic_method`,
    `generic_struct_method`, `nested_captures`, `tuple_members`, and
    `tuple_reverse_closed`; `cargo nextest run --test mir_pass_test
    inline-parametric` lists them all.
  - Bisected to 628ca72a (R662); the test target did not compile in the
    10-29 gate, which hid them.
  - Several cases also changed in ways unrelated to the binder slots, such
    as a `ref.make`/`ref.read` pair that is now one `place.load`; judge each
    one before blessing it.
  - Fix: re-derive each input from current lowering by the recipe in
    `tests/mir_passes/README.md`, then bless and read each expectation.
    Do not bless the current outputs, which read never-stored slots.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1617 A `SIMD` initializer called without inlining has no receiver
  in the VM**

  `Int32(Byte(200))` at module scope in the snippet dialect, or anywhere
  with the pre-elaboration pipeline off, fails with "vm backend does not
  support default/keyword/variadic arguments yet (call passed 1 args to
  2-parameter function 'SIMD.__init__$ov$SIMD$other…')".
  - The instance takes its `out self` as parameter slot zero, as every
    initializer does, but the VM prepends that slot only for a declared
    struct (`Prog::lifecycle_constructor`, `Prog::declared_struct`), and a
    `SIMD` result is the compiler's vector type.
  - Every other call inlines the always-inline initializer first, which
    hides it. `__toplevel__` takes no call-graph pass.
  - `runtime_scalar_conversions_wrap_bit_accurately` in
    `tests/evaluator_test.rs` runs in `main` until this lands.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1398 The checker, VM, and Pliron keep `SIMD` member rules no
  program reaches**

  Once every `SIMD` member is a library body,
  `checker/method_calls/simd_receivers.rs`, the SIMD arms of
  `checker/operators.rs`, `LaneMask::resolve`, `runtime::simd_method`, and
  Pliron's per-method SIMD lowering serve no program.
  - Delete what the member entries left of them.
  - Done when `rg simd_method crates` finds nothing.
  - Depends on R1555, R1589, R1579, R1580, R1581, R1582, R1585, R1586,
    R330, and R276, which port the last members, and R1561, which retires
    the lane subscript.
  - Model: Opus, Not Planned.

- [ ] **R1399 `Int(x)`, `UInt(x)`, and `Float64(x)` are checker built-ins,
  where upstream's are `SIMD.__init__` overloads**

  `checker/builtins.rs` types the conversions by rule, and the VM's
  `builtin_convert` and Pliron's `lower_convert` run them, where upstream
  declares `__init__[T: Intable]`, `__init__[T: IntableRaising](out self:
  Int, ...) raises`, `__init__[T: Floatable](out self: Float64, ...)`, and
  `__init__[T: FloatableRaising](out self: Float64, ...) raises`.
  - Port the overloads, including the refined `out self` they use, and
    delete the scalar arms of all three sites.
  - Porting `Intable` removes a non-`SIMD` argument from
    `Checker::binds_undeclared_overload` (`checker/call_inference.rs`), so
    a sized alias over an `Intable` value takes the declared overload too.
  - That includes its `Bool` clause: a lone `Bool` into a one-lane
    instance whose dtype is not known to be `bool` stays the rule's, where
    the pin selects `__init__[T: Intable]`.
  - Probe: `def h[dt: DType](b: Bool) -> SIMD[dt, 1]: return SIMD[dt,
    1](b)` gives `1` at `DType.int8` and fails "constructing from an
    `Intable` value requires an integral dtype" at `DType.bool` at the pin;
    Mojito rejects both ("expected a dt element, found Bool").
  - Probe: `UInt8(True)` and `SIMD[DType.int, _](True)` print `1` at the
    pin, through `__init__[T: Intable]` (`Bool` is `Intable`). Mojito's
    rule (`check_simd_value`) takes only a struct or a parameter as
    `Intable`. The `scalar.cast`
    instruction (R857) becomes their `pop.cast`.
  - Probe (from R857): a conversion over a struct or a bounded type
    parameter is still a bare-name `call` of `Int`, `Float64`, or `Bool` in
    parametric MIR, as is MIR's `bool_conversion` of a `Boolable` struct
    condition. The elaborator's `Specializer::rewrite_blocks` resolves it by
    name to the dunder's `call.method` or a `scalar.cast`, where upstream's
    `__init__[T: Intable]` body is `value.__int__()`.
  - Probe: `Int(StringSpan("12"))`, which the checker's rule converts
    through `StringSpan.__int__`.
  - Probe: a struct whose `__int__` or `__float__` raises converts through
    `Int(x)` or `Float64(x)` without declaring `IntableRaising` or
    `FloatableRaising` (bundled in `std.builtin.int` and
    `std.builtin.floatable`): the checker's conversion exempts a raising
    dunder from the declared-bound test.
  - Probe: `Bool("abc")` and `"abc".__bool__()` are rejected ("expected a
    numeric or Bool value"), where the pin prints `True`: the built-in
    `Bool(x)` takes only a numeric or `Bool` operand, and a scalar's
    `__bool__`, `__int__`, and `__float__` are checker rules
    (`method_calls/mc_infer.rs`) over the numeric scalars, where upstream's
    `Bool.__init__[T: Boolable]` calls `StringLiteral.__bool__`.
  - Probe: `if "":` and `"" or s` over a `String` are rejected ("expected
    Bool, found StringLiteral"), where the pin tests the literal through
    its `__bool__`: `Checker::condition_truthiness` admits a numeric scalar
    or a struct declaring `__bool__`, and `StringLiteral` is neither here.
  - Probe (from R1109): `Int64(C[4](1))` runs `C.__int__` from its type
    through `simd.make` (`mir::TYPE_REACHED_MEMBERS`), so an `__int__`
    ignoring `self` keeps one instance per `n`, where `Int(C[n](1))`
    already calls it.
  - Depends on R1555, which declares the other `__init__` overloads, and
    R1594 (the refined `out self`).
  - Model: Fable, Not Planned.

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
    the text matches; a hand-written `EnumLike` conformer without its own
    `write_to` still prints the reflective field form.
  - Delete the generation once `Writable`'s defaults are library code.
  - Found while landing `enum` (2026-10-25).
  - Depends on R1461 (`Writable`'s defaults as the trait's bodies).
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
  - Port `_write_scalar`, `write_to`, `write_repr_to`, `write_padded`, and
    `_scalar_repr_alias` into the declared `SIMD` (R1549), which takes a
    `DType` parameter without R160's `DType` struct.
  - Bundle what they import: `std/builtin/format_int.mojo` (`_write_int`)
    and `std/builtin/_format_float.mojo` (`_write_float`). Read R159 first,
    since the float text interacts with it.
  - Retire the inverted-write rule for `SIMD` in `intrinsic_receivers.rs`.
  - Probe: `x.write_repr_to(w)` on an `Int`, `UInt`, or `Float64`
    (`SemanticAdjustment::InvertedReprWrite`, `mir/lower_expr/expr_method.rs`)
    builds `repr(x)` into an owned `String` and writes that, where
    upstream's `SIMD.write_repr_to` writes into the writer itself.
  - Probe: `Int8(4).write_repr_to(t)`, `Float32(1.5).write_repr_to(t)`, and
    `Int8(4).write_to(t)` stop with "type 'Int8' has no method
    'write_repr_to'" (or `'write_to'`), where the pin writes `Int8(4)` and
    `Float32(1.5)`: a sized scalar's host method table has neither.
  - Depends on R1555.
  - Model: Fable, Not Planned.

- [ ] **R359 `Tuple`'s private storage destroys its elements, where
  upstream's storage is trivially destructible**

  Problem: Mojito's `Tuple` holds a `__RuntimeTuple[*Self.Ts]` whose
  destruction destroys every element, where upstream's `!kgen.struct`
  storage has a trivial destructor and `Tuple.__deinit__` destroys each
  element through `Pointer(to=self[i]).unsafe_deinit_pointee()`.
  - The consuming members (`reverse`, `concat`, `consume_elements`) take
    each element with `Pointer(to=self[i]).unsafe_take_pointee()` and then
    must end the storage with `lit.ownership.mark_destroyed`, which upstream
    does not need.
  - Upstream writes the moves as `unsafe_write_move_from` over a rebound
    pointer, which Mojito does not have.
  - `unsafe_take_pointee()` through an origin-bearing pointer to an element
    with a destructor is admitted only in the bundled storage modules
    (`checker/method_calls/builtin_types.rs`), because user code cannot end
    the owner's storage.
  - Depends on R341.
  - Model: Fable, Planned.

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

- [ ] **R341 `Tuple`'s pack, copy, and move initializers store the private
  storage whole where upstream marks `self` initialized and writes each
  element through a pointer**

  Problem: Mojito's `Tuple.__init__(out self, var *args)` builds
  `self.storage = __RuntimeTuple(*args^)`, and its copy and move
  initializers are synthesized over the whole storage. Upstream marks
  `self._mlir_value` initialized and writes per element:
  `args^.consume_elements[init_elt]()` with
  `Pointer(to=self[idx]).unsafe_write(elt^)`, `unsafe_write(copy=copy[i])`,
  and `unsafe_write_move_from(Pointer(to=move[i]))`.
  - The default initializer took upstream's shape on 2026-10-05: the
    `lit.ownership.mark_initialized` statement and
    `Pointer(to=self[i]).unsafe_write({})`.
  - Mojito has no `unsafe_write_move_from`.
  - A `@__parameter` closure over the receiver (`init_elt`) is not spelled
    in Mojito.
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

- [ ] **R330 SIMD's lane gathers are checker intrinsics where the pin's
  are `SIMD` methods over `pop.simd.shuffle`**

  Problem: `shuffle`, `slice`, and `join` are typed by rule in
  `simd_receivers.rs`, and their constraints are checked by
  `LaneMask::resolve`, where the pin declares them in `std/simd.mojo` and
  checks them with `comptime assert`.
  - Join's template mask is the structured `LaneMask::Join`, where the
    pin's is the compile-time result of `indices()`.
  - The pin's `slice` lowers to `llvm.vector.extract` or an element loop,
    not a shuffle.
  - Port `_shuffle_variadic`, `_shuffle_list`, the variadic `shuffle`
    overloads, `slice`, `insert`, `join`, `interleave`, `split`,
    `deinterleave`, the rotations, the shifts, and `reversed` into the
    declared `SIMD` (R1549), and retire `infer_simd_shuffle`/`slice`/`join`
    and `LaneMask`'s rule use.
  - Admit `pop.simd.shuffle` in `MlirOp`, lowering to `MirInstr::SimdShuffle`.
  - Bundle `std/utils/static_tuple.mojo` (`StaticTuple`) and `std/_plugin`
    (`PluginHooks`, `DefaultPlugin`, and `CurrentPlugin`, which is
    `CompilationTarget.current().stdlib_plugin`, R1562). The bodies read
    `CurrentPlugin.lowers_subvector_intrinsics`, `True` on the CPU plugin.
  - The `slice`, `insert`, `interleave`, rotation, and shift bodies call
    intrinsics that run since R1519, `deinterleave`'s since R1547, and
    `slice` reads `__is_run_in_comptime_interpreter` (R1546).
  - The `IndexList` overloads of `shuffle` wait on an `IndexList` entry.
  - Probe: ``__mlir_op.`pop.simd.shuffle`[mask=tup._mlir_value,
    _type=...](a, b)``, upstream's `_shuffle_list` body, is rejected as
    outside the subset. Its `mask` attribute is a compile-time
    `StaticTuple[SIMDLength, n]`'s storage, and Mojito bundles no
    `StaticTuple`. `MirInstr::SimdShuffle` already computes the op.
  - Closes R331 (the pin's `2 * length` bound). Shrinks R212.
  - Depends on R1562, which bundles `CompilationTarget`.
  - Model: Fable, Not Planned.

- [ ] **R422 `print` and `String(x)` write a struct through a host string
  accumulator where upstream writes through a `Writer` struct**

  Problem: a struct's `write_to` reached from `print` or `String(x)` gets a
  compiler-private string accumulator as its writer, where upstream passes a
  real `Writer` (the stdout buffer, or the `String` itself).
  - A `writer.write(...)` call inside such a body dispatches through the
    bound to `__trait_dispatch.write`, which the specializer clears on the
    accumulator's non-struct receiver (`native/mono/specializer.rs`).
  - The VM answers it in the `Value::Str` arm of `method_call`
    (`backend/vm/invoke.rs`) and Pliron in `lower_str_writer_write`
    (`lower/methods.rs`).
  - A writer that overrides `write` is never the accumulator, so the texts
    match the pin; the shape does not.
  - Probe: the accumulator's `write` takes its values one by one, so the
    elaborator passes it the elements of the collector the call builds
    (`unpack_builtin_collectors`, `native/mono/spread.rs`).
  - Found while landing R414 (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1461 `Writable`'s reflective defaults are synthesized per
  conformer, where upstream's are the trait's own bodies**

  `synthesize_writable_defaults` (`comptime/synth.rs`) gives each conformer
  without a `Writer`-fed member a `write_to` and `write_repr_to` that write
  each field through the bundled `_reflection_write_field_to`, where
  upstream's `Writable` declares both as default bodies over
  `_reflection_write_to` (and `_enum_reflection_write_to` for an
  `EnumLike`).
  - The bundled trait's requirements stay `...` until upstream's body
    checks: `r.field_ref[i](this)` (R77).
  - Each bundled module hosting a defaulted conformer imports
    `_reflection_write_field_to` (`std/span.mojo`, `std/string.mojo`, and
    five more), since the synthesized call names it by its linked name.
  - Probe: printing a conformer whose field is not `Writable` fails with
    "Could not derive Writable - a member field does not implement
    Writable", where the pin's `_field_conforms_to_error` names both
    ("Could not derive Writable for Holder - member field `o: Opaque` does
    not implement Writable"). The helper's assert message must be a
    literal (R1210).
  - Probe: a user trait whose default is upstream's body text stops at
    `r.field_ref[i](this)`; its `comptime WriterType = type_of(writer)`
    checks.
  - Done when the trait declares upstream's bodies and the synthesis, the
    helper, and its imports are gone; R1409's generated enum members go
    next.
  - Found while landing R424 (2026-10-26).
  - Depends on R134, R77, and R1210.
  - Model: Fable, Not Planned.

- [ ] **R1481 `Equatable`'s and `Hashable`'s reflective defaults are
  synthesized per conformer, where upstream's are the traits' own bodies**

  `synthesize_equatable_eq` and `synthesize_hashable_hash`
  (`comptime/synth.rs`) give each conformer without its own `__eq__` or
  hasher-fed `__hash__` a body comparing or hashing each field, where
  upstream's `Equatable.__eq__` and `Hashable.__hash__` are default bodies
  over `reflect[Self]` (`std/builtin/comparable.mojo`,
  `std/hash/hash.mojo`).
  - `reflect[Self]` in a plain struct's method already reads the fields.
  - The bodies read each field through `r.field_ref[i](self)` (R77), and
    assert its conformance with `_field_conforms_to_error`, whose message
    must be a literal (R1210).
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

- [ ] **R1287 `print` is a compiler builtin where upstream's is an
  `@inline(.never)` library function**

  `print` is checked, run, and lowered as a builtin call
  (`Checker::infer_print`, the VM's `dispatch.rs`, Pliron's
  `lower/calls.rs`), where upstream's `std/io/io.mojo` declares `def
  print[*Ts: Writable](*values: *Ts, sep: StringSlice = " ", end:
  StringSlice = "\n", flush: Bool = False, var file: RawFileDescriptor =
  stdout)`.
  - `inline_cost` prices the builtin call at the figure the pin's
    `print(s)` call site costs (`PRINT_COST`, 30, in
    `inline_parametric.rs`), since the call holds none of the pack and
    defaults the pin's call site builds.
  - Probe: `@always_inline def show(x: Int): print("show", x)` inlines
    here and keeps its call at the pin, where a `StringLiteral` value
    takes `print("a")` past the bound.
  - The pin prices `print()` at 24 and `print(s, x)` at 31.
  - Probe: a spread into the builtin `print` (`print(*args)`) is passed as
    its elements (`expand_pack_spreads`), where every other call passes
    the caller's pack itself.
  - Done when `print` is the library function, the caller's MIR holds its
    pack and defaults, and `PRINT_COST` is gone.
  - Probe (from R1109): `W[n].write_to(self, mut writer: Some[Writer])`
    ignoring `self`, over a field reading `n`, keeps one instance per `n`
    where the pin keeps one: display reaches `write_to`, `write_repr_to`,
    and a writer's `write_string` from the value's type
    (`mir::TYPE_REACHED_MEMBERS`), so they keep every argument until
    `print` calls them.
  - The VM formats a `print`, `String`, or `repr` operand by its runtime
    struct name (`format_value`), so it reboxes a struct it holds as its
    one field (`Prog::boxed`) from the operand's static type at the
    builtin call. The bridge goes with the builtin.
  - Found while landing R1075 (2026-10-22).
  - Depends on R424 and R422.
  - Model: Fable, Not Planned.

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
    template beside the entries. The span-backed `_PrecompiledEntries`,
    `format_to_comptime`, and `compile_entries_runtime_no_raises` are
    omitted until R425.
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

- [ ] **R1430 A closure storage's parameters are its enclosing
  declarations' binders, where upstream declares them as the storage
  struct's own**

  The `__storage` struct's `param_decls` are the enclosing binders'
  own `ParamDecl`s, owned by the enclosing function (or by `S` for a
  closure in a method of `S`), where upstream's `liftClosure` declares the
  struct's own parameters, reusing their names.
  - Not observable: the elaborator binds them by identity from the
    receiver's type, and `binder_scope` puts them ahead of the method's
    scope (`mir.rs`).
  - Re-owning them to the storage means rewriting the method body's and
    the fields' binder references to the struct's ids at lowering
    (`closure_storage`, `lower_nested_node`, `mir/nested.rs`).
  - Probe: `generic_frame` in
    `assets/ok/closure_storage_method_binders.mojo`, whose
    `generic_frame$body$__storage` carries `n` owned by `generic_frame`.
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
  - The binder collection is `closure_binder_uses` (`mir/nested.rs`), which
    a capturing closure's storage already uses.
  - A `@parameter` closure, upstream's legacy capturing closure, is not
    this entry's.
  - Probe: `def outer[n: Int]() -> Int:` declaring `def helper() -> Int:
    return n * 2` lowers `outer$helper` with `enclosing: outer`; the pin's
    `helper` is a top-level function parameterized on `n`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1436 The checker names a closure's type by its declaration site,
  and MIR builds its storage struct, where upstream's front end builds the
  struct and types the value with it**

  A capture-list closure's or capturing lambda's checked type is a
  `def(...)` whose environment is `CallableEnvironment::Storage` (its site,
  captures, copyability, and passability), which
  `nested::resolve_closure_types` replaces by the `__storage` struct
  `closure_storage` derives in MIR, where upstream's `liftClosure`
  declares the struct in the front end.
  - Move `closure_storage`'s derivation into the checked program (a checked
    storage declaration on `CheckedDef`), so the value's type is a
    `TyKind::Struct` from the check on.
  - The storage's `ref` fields carry an untracked origin; upstream promotes
    the captures' origins to the struct's origin parameters
    (`outlineAndPromoteOrigins`). The environment's capture set stands in
    for them in the checker.
  - The checker's copyability and passability of the storage duplicate
    MIR's meet over the captures (`closure_storage`, `mir/nested.rs`).
  - Found while landing R1378 (2026-10-25).
  - Depends on R1430.
  - Model: Fable, Not Planned.

- [ ] **R1434 A closure keeps owned captures, which no pin-shaped value
  needs once a closure's value is its storage**

  A capturing lambda converted to a run-time callable moves its storage
  into a one-capture `closure.make` (`MirCaptureMode::Move`,
  `lambda_closure` in `mir/lower_expr/calls.rs`), the only producer of an
  owned capture.
  - Its consumers exist for that shape alone: the VM's owned-capture
    `RefProjection::Capture` and `callee_place` paths
    (`closure_capture_arguments`, `backend/vm/references.rs`), and
    Pliron's value capture slots and `mjdrop_<n>` thunks
    (`lower/closures.rs`).
  - Delete them once no run-time `capturing[...]` value holds a moved
    storage.
  - Once deleted, `mem-2-reg`'s call-through rule (`Facts::candidate`,
    `analysis/mem2reg.rs`) can admit every function-typed slot, not only
    a thin one or a local holding a closure of reference captures: a
    call through a parameter of a `capturing` function type still keeps
    its `callee_place` and the slot it names.
  - `canonicalize`'s `fold_known_callees` folds a call through a closure
    of reference captures only: a closure holding a lambda's moved
    storage has no place to pass as its body's `self`, so a call through
    `closure.make` of a `$lambda` storage method stays `call.indirect`.
  - A capturing closure's register is owned (`owned_register`,
    `mir/register_ends.rs`) for the same reason, so a closure of
    reference captures keeps its `drop.reg` until `canonicalize` erases
    it once its calls are folded.
  - Found while landing R1378 (2026-10-25).
  - Depends on R1433, R176.
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

- [ ] **R937 `**` on integers and scalar displays call bundled bodies
  that no instruction names**

  `**` on `Int`/`UInt` calls `_pow_int`, and a scalar display calls the
  digit bodies, but the MIR instruction names neither, where upstream calls
  `Int.__pow__` and `write_to`, whose callees are ordinary symbol uses.
  - The elaborator, Pliron's reachable set, and `eliminate-dead-symbols`
    each read the edge from `mir::implicit_callees`, a rule outside the
    symbol walk.
  - The fix lowers each to a call that names its callee, then deletes
    `implicit_callees`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1491 A variadic value parameter is one binder where upstream
  declares two**

  Upstream's `*vs: T` declares an implicit infer-only `vs.values:
  !kgen.param_list<T>` binder and the visible `vs: ParameterList[T,
  vs.values]` (`typeCheckVariadicParams`, `Signatures.cpp`), where
  Mojito declares the one `ParamDecl::Value { ty: ParamList(T), variadic:
  true }` and re-spells `ParameterList[vs.values]()` at each run-time
  read (`infer_value_pack_read`).
  - The hidden binder goes after the existing infer-only parameters, as
    `addImplicitTypeParams(append=false)` inserts it, so slot numbering,
    mangling, and a struct's identity gain the list slot.
  - Bracket arguments bind the hidden list through the visible binder's
    element type (`getParameterListInfo`, `ParamInf.cpp`).
  - Then a pack and a raw-list binder differ by declared type, as
    upstream's do, and `value_pack_in_scope`'s pack-scope test goes.
  - Probe: `var x: String = vs` in `def f[*vs: Int]` reports "found
    ParameterList[Int, vs]"; the pin says 'ParameterList[vs.values]'
    (R1489's value half).
  - A session should plan this first and split it if it does not fit.
  - Found while landing R1151 (2026-10-27).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1492 A variadic type parameter is one binder where upstream
  declares two**

  Upstream's `*Ts: Tr` declares an implicit infer-only `Ts.values:
  !kgen.param_list<Tr>` binder and the visible `Ts: TypeList[Tr,
  Ts.values]`, where Mojito declares the one `ParamDecl::Type { variadic:
  true }` and re-spells `TypeList[Ts.values]()` at each run-time read.
  - The same split as R1491, for type packs: the `TypeList` half of the
    spelled pack read goes, and so does `pack_parameter_in_scope`.
  - Probe: `var y: Int = Ts` in `def go[*Ts: Movable]` reports "found
    TypeList[Movable, Ts]"; the pin says 'TypeList[Ts.values]' (R1489's
    type half).
  - A session should plan this first and split it if it does not fit.
  - Found while landing R1151 (2026-10-27).
  - Depends on R1491.
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

- [ ] **R1607 `OwnedPointer` lacks upstream's refined initializers**

  `OwnedPointer(copy_value=s)` matches no initializer, where the pin copies
  `s` into a new allocation.
  - Upstream declares four `__init__[_T](out self: OwnedPointer[_T], ...)`
    overloads (`var value`, `copy_value=`, a copying `value` ranked lower by
    a dummy `U: NoneType = None` binder, and `other=`), which define `T`
    through the refined `out self` R1594 supports.
  - The bundled `stdlib/std/memory/owned_pointer.mojo` declares one
    `__init__(out self, var value: Self.T, /) where conforms_to(Self.T,
    Movable)` instead.
  - Upstream's `other=` body reads `other[]`, the empty subscript Mojito
    reserves for raw pointers (a subset gap the bundled file records).
  - Probe: `var p = OwnedPointer(copy_value=String("hi"));
    print(p.unsafe_ptr()[0])` prints `hi` at the pin.
  - Found while landing R1594 (2026-10-28).
  - Depends on nothing.
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
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1122 `Tuple(init_with=...)` constructs each element in place**

  Problem: `T(init_with=lambda [i: Int]() -> T.Ts[i]: Pinned(i * 10))` is
  rejected, where the pin builds each element from a closure parameterized
  on the element index, which is how a tuple of non-movable elements is
  made at all.
  - The handler is a callable parameter with its own binder and a
    dependent result type, `F: def[idx: Int]() -> Self.Ts[idx]`.
  - Depends on R1121.
  - Model: Fable, Not Planned.

- [ ] **R1123 `Tuple.first_of[T]()` returns the first element of type `T`**

  Problem: `t.first_of[String]() += "!"` is rejected ("no attribute
  'first_of'"), where the pin resolves the lookup at compile time and
  returns a reference, erroring when no element has that type.
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1124 `consume_elements` and `deinit_with` take the handler as a
  runtime closure argument**

  Problem: the bundled `Tuple.consume_elements[handler]()` is a
  compile-time parameter, where the pin takes `t^.consume_elements(handler)`
  for `def handler[idx: Int](var elt: t.Ts[idx]) {mut collected}` and
  rejects the parameter spelling ("missing required argument:
  'elt_handler'"); `Tuple.deinit_with` and `VariadicPack.consume_elements`
  changed the same way, matching `List.deinit_with`.
  - `conformance/fixtures/tuple_consume_elements.mojo` and the three
    `assets/type_error/tuple_consume_elements_*.mojo` keep the parameter
    spelling until this lands.
  - The handler is a generic callable argument whose binder indexes the
    receiver's pack, as R1122's is.
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
  - Depends on nothing.
  - Model: Fable, Not Planned.

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

- [ ] **R1226 `Span` does not declare `TrivialRegisterPassable`**

  Problem: `ref r = values[1:]` over a `List[Int]` runs, where the pin
  rejects it with "value of type 'Span[...]' doesn't have a memory origin
  in 'ref' binding", because upstream's `Span` is
  `TrivialRegisterPassable` (`Mojo/stdlib/std/collections/span.mojo`).
  - The bundled `Span` (`stdlib/std/span.mojo`) declares only
    `ImplicitlyCopyable` and `Movable`, so the checker's register-passable
    `ref` rejection never fires for it.
  - Declaring it changes how a `Span` passes natively, so the native ABI
    rows in `docs/native-abi.md` must follow.
  - Probe: `conformance/probes/span_ref_binding_register_passable.mojo`,
    which also holds the starred `case [f, *ref rr]` the pin rejects the
    same way.
  - Watch R860: the checker does not yet enforce
    `TrivialRegisterPassable`'s rules at a declaration.
  - Found while planning R1137 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1154 `List` has no constructor from an iterable**

  Problem: `List(xs[1:3])` is rejected ("no constructor overload matches
  the supplied arguments"), where the pin copies the iterable's elements
  into a new list, the way Mojo owns a slice.
  - Upstream's overload is `__init__[IterableType: Iterable](ref iterable:
    IterableType, out self: List[IterableType.IteratorType[
    origin_of(iterable)].Element])`.
  - The parser takes a method's receiver only as its first parameter
    (`parser/items.rs`: "a receiver convention must be followed by
    'self'"), so the overload does not parse. Its refined `out self`
    annotation itself is supported (R1594 landed); only the position
    after another parameter, whose type the refinement reads, remains.
  - Probe: `conformance/probes/list_from_iterable.mojo`.
  - Found while landing R190 (2026-10-19).
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
  - Found while landing R665 (2026-10-13).
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
  - Done when they and `ParameterList` use upstream's builder text.
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
  `Pointer(to=result[i]).unsafe_write(self.storage[k]^)`, where upstream
  writes `Pointer(to=result[i]).unsafe_write_move_from(rebind[Pointer[
  type_of(result[i]), origin_of(self)]](Pointer(to=self[k])))`.
  - A move out through a pointer is untracked, and the compiler destroys the
    `__RuntimeTuple` storage, so the element would be destroyed twice.
  - `concat` retypes the destination place instead of the source pointer:
    `rebind[Self.Ts[i]](result[i])`.
  - `concat` reads its length as `Self.Ts.length` where upstream calls
    `Self.__len__()`.
  - The loop headers read `Self.Ts.length` and `OtherTs.length` where
    upstream reads `type_of(result).__len__()` and
    `type_of(other).__len__()`.
  - Pinned by `assets/ok/tuple_reverse_concat.mojo`.
  - Depends on R341 (the storage model and `unsafe_write_move_from`), R1509
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

- [ ] **R212 A struct's own value parameter as a SIMD width in a signature
  is rejected as a type**

  Problem: `def zeros(self) -> SIMD[DType.int64, Self.length]` reports `SIMD
  width must be a positive power of two, got a type`, where upstream binds
  the parameter.
  - Cause: the width slot needs a concrete comptime `Int`.
  - Depends on nothing.
  - Model: Opus, Planned.

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

- [ ] **R221 `Pointer.unsafe_bitcast[U]()` leaves its result at the old
  element type**

  Problem: `Pointer.unsafe_bitcast[U]()` is not typed: an origin-cast-style
  forwarding leaves the MIR register at the old element type.
  - Probe: upstream's `VariadicList` constructor ends with
    `.unsafe_bitcast[Self._EltPointerType]()` over the array's first
    element (R1317).
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
  - An unconditional `Iterable` also needs R178: the bundled `Iterable`
    requires `comptime Element: Copyable & Movable`.
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
  - Probe: an overloaded free function hits it too: `def f(value:
    StringSlice)` beside `def f(value: Int)` fails on the VM with "does not
    support the built-in or callee 'f$ov$StringSpan'"; the bundled `len`'s
    `@unavailable` overload spells `StringSpan` for it.
  - Cause: the declaration's symbol mangles the annotation's spelling
    (`signature_from_ast`), the call's the type it resolves to
    (`TypeKey::from_ty_with_self`), and an alias spells neither the same.
  - Probe: a member alias does too. `struct P[n: Int]` with `comptime T =
    SIMD[DType.int32, Self.n]` and `__init__(out self, *, raw: Self.T)`
    beside `__init__(out self)` fails on the VM with "keyword arguments to
    'P.__init__$ov$SIMD$DType$int32$$n$$kw$raw' are not supported", where
    the pin runs `P[2](raw=SIMD[DType.int32, 2](5))`. `SIMD`'s
    `mlir_value: Self._mlir_type` overload hits it (R1591).
  - Depends on nothing.
  - Model: Fable, Not Planned.

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

- [ ] **R331 A one-operand `shuffle` rejects a lane index at or above the
  receiver's width, which the pin accepts**

  Problem: `SIMD[DType.int32, 2](5, 6).shuffle[3, 0]()` fails to check
  with "expected a lane index below 2", where the pin prints `[6, 5]`.
  - The pin's one-operand `shuffle` gathers from the receiver joined with
    itself and asserts only `0 <= mask[i] < 2 * Self.length`.
  - The checker's rule is in `infer_simd_shuffle`, and an instance's in
    `LaneMask::resolve`; both should take the pin's bound and fold an
    index `i >= w` to `i - w`.
  - `tests/checker_test.rs`'s `rejects_bad_simd_shuffle_masks` pins the
    current rejection.
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R429 `Error` has no variadic `Writable` constructor**

  Problem: `Error("Index ", 3, " not in *args")` builds the joined message
  at the pin, while Mojito reports "'Error' expects 1 argument(s), got 3".
  - Upstream declares `Error.__init__[*Ts: Writable](out self, *args: *Ts)`.
  - Probe: `String("some ", x)` has the same gap ("'String' expects 1
    argument(s), got 2"); the pin prints `some 3`
    (`conformance/probes/match_optional_enum_cases.mojo`).
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

- [ ] **R850 Float SIMD `%` is rejected**

  Problem: `Float32(7.5) % Float32(2.0)` fails with "operator '%' is not
  defined for Float32 and Float32", while the pin prints `1.5`.
  - Float `//` runs, as upstream's `pop.floordiv` (R1392).
  - Upstream's float `__mod__` is a library body in `std/simd.mojo`: the
    quotient truncated, then `self - div * rhs`, plus `rhs` when the signs
    differ. Each step rounds to the lane.
  - Once the VM computes it, add it to `ct::float_lane_binop`, so `sccp`
    folds it too.
  - Closed by R1623, which ports upstream's `__mod__`.
  - Found while landing R846 (2026-10-14).
  - Depends on R1623.
  - Model: Opus, Not Planned.

- [ ] **R1410 `String`'s variadic constructor is rejected**

  `String("word:", name)` stops with "'String' expects 1 argument(s), got
  2", where upstream's `String.__init__[*Ts: Writable](out self, *args:
  *Ts)` writes every argument, as the pin's enum integration test does.
  - Probe: `print(String("span:", 3, "..", 7))` prints `span:3..7` at the
    pin.
  - Found while landing `enum` (2026-10-25).
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
  - If Python interop is out of scope, this moves to `docs/non-goals.md`.
  - Found while landing R578 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1564 The bundled `List` has no `__getitem__` over an `Indexer`**

  `l[Int32(1)]` over a `List[Int]` is rejected ("no overload matches the
  supplied arguments"), where the pin prints the element.
  - Upstream's `List` declares `__getitem__(ref self, idx: Some[Indexer])`
    beside the `Int` overload, and every sized integer scalar is an
    `Indexer` since R1550.
  - Port the overload into `stdlib/std/collections/list.mojo`, then check
    `Span`, `Array`, and `String` subscripts against upstream's.
  - Found while landing R1550 (2026-10-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Packaging, Artifacts, And Developer Tooling

Track: `tooling`.

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
