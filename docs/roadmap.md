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

Next free ID: **R1448**.

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

  `struct S[n: Int](Copyable, Movable)` holding `var s: String`, built
  once in `main`, stops with "after pre-elaboration pass
  `inline-parametric`: MIR function 'S.__copyinit__' removes parameter
  slot 0 of `S`, which it reads", where the pin runs it.
  - `remove-unused-params` removes `n` from the synthesized
    `S.__copyinit__`, which reads none of it.
  - Inlining `String.__copyinit__` splices places rooted at the bare
    receiver `S` (`$v1.s.size`, `$v1.s.data`, an `index.get` over it).
  - The parameter-use walker (`mir::unread_parameters_of`) then counts
    `n` as read.
  - Suspect: a bare self spelling with no struct arguments reads every
    slot (`Walker::serialize_seq`).
  - Probes: the same over `T: AnyType`, an `@always_inline("nodebug")`
    written copy constructor, and `S[n](copy=c)` in a generic `dup[n]`.
  - A `Movable`-only `S` runs.
  - Fails at `b93e0128`, before R1050.
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

- [ ] **R1379 A closed local `comptime` value binding is folded by the
  elaborator above the check**

  `Elab::stmt` folds a local `comptime` binding that names no binder and
  applies no callable through `Elab::eval` (`comptime/eval.rs`), where
  upstream builds every local `comptime` as a typed parameter expression
  (`StmtParser::parseAliasDeclStmtBody`,
  `getParamEmitter(EC_AliasValue).emitExprPValue`).
  - The lever: `keep_template_comptime_binding` keeps every value binding of
    a function body. The check binds it through `bind_template_comptime`,
    and `lifted_application` stops leaving a closed expression to the
    elaborator.
  - The template route already serves these shapes: `comptime x = n + 1 if c
    else n` and `comptime d = UInt8(n)` inside `def g[n: Int]` both run.
  - Probe: R1245's `t[1] if c else t[0]` and R853's `comptime d =
    UInt8(250)` in `main` are rejected. The pin prints `5` and `250`.
  - Closes R1245 (a closed conditional is rejected) and R853 (a sized-scalar
    local binding is refused). Shrinks R518 (its local form) and R1191 (a
    local string constant).
  - Depends on R386, so a closed `String` or float binding is lifted instead
    of regressing.
  - Model: Opus, Not Planned.

- [ ] **R1380 A local `comptime` type alias is substituted by the
  elaborator's alias table**

  The elaborator puts a body's `comptime S = <type>` in the block's
  `TypeAliases` and rewrites the statements after it, where upstream
  resolves the alias by name lookup to its parameter value.
  - The lever: every local alias goes into the check's `local_type_aliases`,
    as template bodies' aliases already do.
  - `comptime_type_operand` (`checker/comptime_validation.rs`) learns
    `Scalar[dt]`, `SIMD[dt, n]`, `Self`, and `Self.<type member>`. A
    `Self.<value>` goes to R1379's value route.
  - The block-local `TypeAliases` substitution goes. The pack-element
    spelling stays for the pack entries (R1169).
  - Probe: `comptime S = Scalar[DType.float64]`, then `S(2) * 1.25`. The pin
    prints `2.5`. Mojito fails, in `main` and in `def g[n: Int]` alike.
  - Closes R475 (a local `Scalar` or `SIMD` alias), R476 (`comptime k =
    Self.n` read as a type), and R1170's alias half.
  - Depends on R1379, whose value route takes what this route declines.
  - Model: Opus, Not Planned.

- [ ] **R1381 A closed module constant is folded by `Elab::eval`, not by the
  check's parameter-expression folder**

  Closed module constants are folded before the check by `Elab::eval`, with
  its own name resolution, where upstream emits a module `comptime` through
  `IREmitter` in `AliasASTDecl::resolveSignature` (`DeclResolution.cpp`).
  - The lever: `Elab` hands every module value constant to the check. The
    check's pre-pass builds each one with `compile_dependent_ct_expr` and
    folds it with `param_expr::fold`, and anything left over becomes the
    `ParamKind::Apply` request applied constants already use.
  - The elaborator's `top_consts` values, `fold_default_bindings`' constant
    reads, and the crossing folds over constants go.
  - Merge `program_constants`, `comptimes`, `module_values`, and
    `comptime_literals` into one table.
  - Probe: R135's module `comptime K = Int32(4)` is refused. The pin prints
    `4`.
  - Closes R135 (a sized-scalar module constant) and R518's constant half.
    Shrinks R1191 and R1171.
  - Depends on R1379.
  - Model: Fable, Planned.

- [ ] **R1382 Module type aliases and generic aliases live in an elaborator
  table the package exports**

  `TypeAliases`, `generic_aliases`, and `apply_generic_alias` resolve module
  aliases above the check, and a package build exports them as
  `ElaboratedConstant`, where upstream holds an alias only as its
  `AliasDeclOp`.
  - The lever: the check's `comptime_aliases` is the only table, and an
    importer reads a package's aliases from the check's installed
    interfaces.
  - `apply_generic_alias`, `apply_generic_alias_values`,
    `keyword_application_params`, the alias `check_where_clauses` path,
    `elaborate_exporting`'s constants, and the `ElaboratedConstants` type
    go.
  - Probe: `comptime MyTr = Copyable & Movable`, then `g[MyTr]()`, stops at
    "'Copyable' is not a compile-time type". The pin prints `1`.
  - Shrinks R1171 to its checker half (a trait-valued constant).
  - Depends on R1380 and R1381.
  - Model: Fable, Planned.

- [ ] **R1383 A reflection query is answered by the elaborator above the
  check**

  `Elab::eval_reflection_method`, `eval_reflected_field_handle`,
  `resolve_reflected_type`, and the crossing pass's `reflection_query` fold
  `reflect[...]` before the check, where the check (`checker/reflection.rs`)
  and `ParamKind::Reflect` already answer the same queries through
  `ReflectQuery::answer`.
  - The lever: a reflection handle in any position is the check's, and only
    the check and mono answer it.
  - Probe: `comptime r = reflect[Self]` in a plain struct's method fails
    with "unsupported compile-time type argument". The pin prints the field
    count.
  - Closes R269 (`reflect[Self]` in a plain struct). Unblocks R324.
  - Depends on R1379.
  - Model: Opus, Planned.

- [ ] **R1384 A struct-valued bracket argument is frozen to its fieldwise
  construction above the check**

  `Elab::freeze_struct_value_arguments` evaluates `Tagged[Extent.square(4)]`
  to `Tagged[Extent(4, 4)]`, where upstream keeps the argument symbolic as a
  `kgen.param.apply` its elaborator interprets.
  - The lever: `lifted_application` already lifts a struct-valued call in a
    body. Signatures and field types take the `called_application` arm, and
    mono freezes the result (`VmBackend::freeze`).
  - `freeze_struct_value_arguments`, `freeze_arguments`, and
    `explicit_parameters` go.
  - Probe: R1175's `T[k=3, Extent.square(2)]`, which prints `23` only
    because CTFE later evaluates the construction the freeze failed on.
  - Shrinks R1175 (its `freeze_arguments` bullet).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1385 A struct's associated `comptime` member that applies a
  callable is not a parameter expression**

  `Checker::eval_associated_ct` refuses `comptime V = f(Self.k)` with "not
  an associated comptime expression", where upstream emits a member through
  the same `EC_AliasValue` emitter as any alias, with an apply the
  elaborator runs per instance.
  - The lever: the applied-constant shape over the generator's binders. The
    fallback arm binds a call as `ParamKind::Apply` and reads
    `Self.<sibling>` through the generator.
  - `Elab::associated_value_at` then has no reader the check does not own.
  - Probe: R489's `S[3].get()` returning `Self.V`. The pin prints `4`.
  - Closes R489 (a member applies a function), R802 (a `len(Self.values)`
    member), R518's member bullet, and R1206's sibling probes. Shrinks R1178
    and R1194.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1386 Bracket arguments and associated members are evaluated by a
  second per-shape folder**

  `eval_associated_ct` (`checker/constraints.rs`) repeats
  `compile_dependent_ct_expr`'s arms with its own name lookup, where
  upstream checks each compile-time expression once into a typed parameter
  value.
  - The lever: `eval_associated_ct` becomes `compile_dependent_ct_expr`
    followed by `param_expr::fold`.
  - `member_type_operand` reads a member through a construction's type.
  - Probe: R1191's `bind[lit]()` over a string constant fails with "not a
    compile-time Int constant". The pin prints `So`.
  - Closes R1191 (a string constant in a bracket) and R1194 (a member read
    through a `TypeList` construction). Shrinks R1199.
  - Depends on R1385.
  - Model: Fable, Planned.

- [ ] **R1387 A type operand in a condition and in a value are built by
  different evaluators**

  `constraint_operand` reads `T == Int` in a `comptime if`, the value
  evaluators read `T` as a variable, and `comptime_type_operand` is a third
  reader, where upstream's type comparison is one parameter operator over
  type-valued parameter expressions.
  - The lever: `constraint_operand` and `comptime_type_operand` take their
    operands from R1386's builder, which then returns type-valued
    expressions too.
  - Probe: R1180's `comptime b = T == Int` in `def g[T: Movable]` fails with
    "Undefined variable 'T'". The pin prints `True`.
  - Closes R1180 (a type comparison as a `Bool`) and R1170's `comptime if
    Self.First == Int`. Shrinks R1178 (the `W[U].Get[0] == Int` site).
  - Depends on R1386.
  - Model: Fable, Planned.

- [ ] **R1388 `mojito-comptime` still carries a compile-time evaluator and a
  type resolver**

  Once every site is the check's, the elaborator above the check still holds
  `comptime/eval.rs`, `type_from_name`, `type_from_anno`, `resolve_ct_arg`,
  `param_arg_type`, `associated_value_at`, `generator_member`, and the
  remaining closed-literal crossing folds, where upstream evaluates
  parameter expressions only in its elaborator, after sema.
  - Delete them. The elaborator's `ConformanceOracle`, built only for
    `Elab`, goes too.
  - The type-member construction rewrite (`S.Of[Int]()`) goes once R603
    resolves every alias receiver.
  - Update `docs/architecture.md`'s compile-time-call and
    checker-interaction sections.
  - Probe: R1179's `struct P[*Ts: Movable] where Ts.length == 2` stops while
    the oracle is built. The pin prints `2`.
  - Closes R1179 (the oracle stops the elaborator). Shrinks R1175
    (`type_from_name` and `bind_subscript_arguments`) and R207's elaborator
    bullet.
  - Done when `rg 'fn eval\b|type_from_name' crates/mojito-comptime` is
    empty.
  - Depends on R1380, R1382, R1383, R1384, and R603.
  - Model: Opus, Not Planned.

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

- [ ] **R1342 `TypeList` pack queries over a concrete pack keep their
  inlined call**

  `GenericConstraint::PackPredicate`, `PackContains`, and the
  `ConstraintOperand::PackLength` operand name a pack binder, so a
  `nodebug` callee testing one at a concrete pack keeps its call:
  `replace_constraint_parameters` fails it with "a proposition over `Ts`
  at a binding that is no parameter".
  - Upstream's `TypeList` queries take the type list as an attribute
    operand, so substituting the pack yields the bound list.
  - Lever: an operand on each, as R1096 gave `ConformsPack` (a pack binder,
    or the bound list as `operand_value`/`operand_expr`); once none is
    binder-keyed, `GenericConstraint::try_map`'s `binder` closure goes.
  - Probe: `has_int[Int, String]()` over a `nodebug`
    `def has_int[*Ts: AnyType]() -> Int: return 1 if
    TypeList[Ts.values]().contains[Int]() else 2`.
  - Probe: `TypeList[Ts.values]().all[IsTriviallyCopyable]()` in the same
    shape (`pack_predicate`).
  - Probe: `comptime if TypeList[Ts.values]().length == 2` in the same
    shape (`operand_pack_length`).
  - Each prints `1 2` on the pin and in Mojito; done when a pass test
    under `tests/mir_passes/inline-parametric/` inlines each.
  - Found while landing R1096 (2026-10-24).
  - Depends on nothing.
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

- [ ] **R1344 `mem-2-reg` keeps the slot of a closure a call goes
  through**

  A nested `def` passed as a callable binder or bound to a local stays in
  a slot, so a call of it that is not inlined reads `var.use` of the slot
  and stays `call.indirect` until the elaborator resolves it per instance,
  where upstream's `Mem2Reg` promotes the closure and its canonicalizer
  then folds the call to the closure's function.
  - `call.indirect`'s `callee_place` names the slot, which keeps it
    (`mem2reg.rs`: "a call through the slot keeps it"); `canonicalize`
    folds only a callee register that `closure.make` itself defines.
  - Lever: decide what `callee_place` still records for a closure value
    (loans over its captures), and promote the slot when nothing needs
    it, so the call reads the `closure.make` register.
  - Probe: `print(apply[inner](3))` over a capture-free nested
    `def inner(x: Int) -> Int` and a `nodebug`
    `def apply[f: def(Int) thin -> Int](x: Int) -> Int: return f(x)`
    keeps a `var.store` of `closure.make "main$inner"` and a `var.use`
    feeding a `call.indirect` in `emit-mir`'s `main`.
  - Probe: `var g = inner` then `print(g(1))` keeps the same slot.
  - A `nodebug` nested `def` already inlines through the slot
    (`inline-parametric`'s `closures.called`), so this is the call's
    shape, not inlining reach.
  - Found while landing R1098 (2026-10-24).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1345 `canonicalize` keeps a `call.indirect` through a capturing
  closure**

  A `call.indirect` whose callee register is a `closure.make` with
  captures stays indirect, where upstream's
  `CallIndirectOp::canonicalize` folds a `create_closure` callee into a
  call of its function with the captures prepended to the arguments.
  - A direct call of a capturing nested `def` already has that shape:
    each captured place is a leading `args` operand, with its `arg_places`
    entry and `capture_accesses`.
  - Lever: build those operands from the `closure.make`'s `captures` in
    `fold_known_callees` (`analysis/canonicalize.rs`): a load of each
    reference capture's place and its access. A first-class use of a
    capture-list `def` captures its storage by reference since R1377, so
    it folds the same way; a lambda used as a value moves its storage into
    its closure since R1427, so the fold passes the closure's storage as
    the body's `self`.
  - Probe: `outer` in
    `tests/mir_passes/canonicalize/indirect_known_callee.mir` keeps its
    `call.indirect` through `closure.make "outer$plus"`.
  - Found while landing R1098 (2026-10-24).
  - Depends on R1344, which makes the shape reachable from source.
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

- [ ] **R1031 The VM still dispatches an indirect call on a struct value
  by name**

  `CallIndirect` over a `Value::Struct` callee looks `__call__` up through
  `Prog::selected_struct_member`, though no checked program reaches it.
  - R1009 made the checker reject a callable struct where a `def(...)`
    value is expected, so a struct callee is only ever a direct call the
    elaborator devirtualizes into a `MethodCall` naming the instance.
  - Delete the `Value::Struct` arms in `backend/vm/frames.rs` and
    `backend/vm/exec.rs` (and `nominal_receiver`), so a struct callee fails
    as any other non-callable value, after confirming the elaborator's
    devirtualization never `continue`s for a checked program.
  - Done when the arms are gone and the HEAD differential is clean.
  - Found while landing R1009 (2026-10-15).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1286 `inline_cost` prices a `for` loop's `__next__` as its own
  body, where the pin counts it inlined into the loop's `try`, its
  iterator left in memory**

  Three loop shapes cost 31 or fewer by `inline_cost` and keep their call
  at the pin, which counts at least 32 operations.
  - Probe: `for i in range(x): break` costs 31 here.
  - Probe: `for i in Bounded(x): s += i`, over a struct iterator whose
    `__next__` tests `self.i == self.n`, raises `StopIteration`, and
    stores `self.i`, costs 31 here (`var it = Bounded(x)` first costs 33).
  - Probe, once R1288 lets a callee holding a `try` inline: `var r =
    range(x)`, then `try: s += Int(r.__next__())` costs 27 here.
  - Since R890 the first two keep their call here too, but only because
    each loop holds the `try` around its `__next__` call, which R1288's
    veto refuses; once R1288 lands, the cost decides them again.
  - The pin's count stays small for a raising inlined body that does not
    store to `self` (11) and for one that stores without raising (5), so
    the operations it adds are a slot its `Mem2Reg` leaves in memory.
  - The `__next__` call is inlined into the loop's `try` before its
    caller is measured (R890), so `inline_cost` can price the slot
    traffic left after `mem-2-reg`, as the pin's count holds it;
    `protocol_call_cost` now prices only the `__iter__` call.
  - Done when the probes decide as the pin does and `protocol_call_cost`
    is gone.
  - Found while landing R1075 (2026-10-22).
  - Depends on R1288.
  - Model: Opus, Not Planned.

- [ ] **R1288 `inline-parametric` keeps the call to a callee holding a
  `try`, which the pin inlines**

  `is_closed` refuses a callee whose body holds a `try`, `nodebug` or
  plain, where the pin inlines both below its size bound.
  - Probe: `@always_inline def f(x: Int) -> Int` holding `var s = x`,
    `try: s += 1`, `except: pass`, and `return s` inlines at the pin,
    `nodebug` too.
  - Probe: the same `try` around `s += g(x)`, over an `@always_inline` `g`
    that raises `StopIteration` when `x == 3`, inlines at the pin, costing
    10 there.
  - Probe: since R890 every `for` loop over a raising iterator holds a
    `try` around its `__next__` call, so `@always_inline def first(x:
    Int) -> Int` holding `var s = x` and `for i in It(x): s += i; break`,
    over an `It` whose `__next__` never raises, costs 19 here and inlines
    at the pin, but keeps its call here
    (`tests/mir_passes/inline-parametric/for_protocol.mir` pins it kept).
  - Probe: R1280's `@always_inline("nodebug") def count[T: Iterable]`
    counting `for _ in a` keeps its call in `fc` here since R890.
  - Probe: after elaboration, `automatic-inline` keeps the same calls: a
    plain `def` holding a `try`, or a `for` loop over a raising iterator,
    called from `main` stays a call in concrete MIR, where upstream's
    `AutomaticInline` inlines it below its threshold of 50 operations.
  - The splice would place the callee's `try` in the caller's block, its
    parts sharing the caller's registers as an inlined `if`'s do.
  - `inline_cost` already prices a `try` as four beside its parts.
  - It also prices a handler that its body cannot reach, which the pin
    empties: `try: s += 1`, `except: s += 2` costs 6 there and 8 here.
  - The comments on `is_closed` and in
    `docs/pre-elaboration-optimization-plan.md` say the pin refuses such a
    callee, which neither the 2026-09-21 nor the 2026-10-17 pin does.
  - Done when the probes inline and a pass case pins one.
  - Once this lands, a homogeneous `read` collector a `for` body iterates
    (`total(1, 2, 3)` in `assets/ok/always_inline_nodebug_collectors.mojo`)
    inlines as a lent slot, as the loop-free `cnt` does since R1099; a
    `for` body's place-bound receiver composes through its `$iter` slot,
    so `rebind_blocks`' whole-root `GetIter` rule needs no change.
  - Found while landing R1075 (2026-10-22).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1255 MIR has no select instruction, so `canonicalize` cannot turn
  an `if` into one**

  The generator `Canonicalizer` rewrites an `hlcf.if` whose arms only
  yield values into `pop.select` (`EmptyIfToSelect`, `IfToSelect`,
  `IfYieldSelect`, `lib/Transforms/Canonicalizer.cpp`), and Mojito keeps
  the `if`.
  - Shape: an `if` whose two arms yield different constants, such as
    `x = 1 if c else 2`, stays an `if` with a result; `HoistYieldResults`
    folds only equal yields and a `Bool` true/false pair.
  - The fix adds a select instruction to MIR, the VM, and Pliron, and the
    three patterns to `canonicalize/hlcf.rs`.
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

- [ ] **R1109 A member a backend reaches from a value's type keeps its
  unused arguments**

  `E[n].__eq__(self, other: Self)` returning `True`, `W[n].write_to` that
  ignores its `self`, or `Arr[n].__getitem__(self, i: Int)` returning `i`
  keeps one instance per `n`, where upstream's pass clones the generator
  without the argument.
  - The backends run a table member (`mir::DISPATCHED_MEMBERS`: `__eq__`,
    `__hash__`, `write_to`, `__int__`, `__call__`, `__setitem__`, …), an
    iteration's `__iter__` and `__next__`, and the dunders the builtins
    dispatch (`__len__`, `__abs__`, `__bool__`, `__round__`, `math`'s) from
    the value's type with every argument, and every subscript passes its
    accessor's receiver, so `mir::unused_arguments` gives them no candidate
    (`PROTOCOL_MEMBERS`, `SUBSCRIPT_ACCESSORS` in `mir/unused_args.rs`).
  - Upstream has no such dispatch: each is a call in generic library code
    that `RemoveUnusedParams` rewrites. The fix either makes each reach a
    call the elaborator rewrites, or has the backends' table dispatch drop
    the operands the entry's instance removed, with the member table
    recording the reduced instance.
  - Probes: `__call__(self, x: Int)` ignoring `x` through a callable value
    (the devirtualized `call.indirect` already drops, R1031 holds the rest);
    `__setitem__(self, i: Int, v: Int)` ignoring `v`; `__iter__(self)`
    returning an iterator built from nothing of `self`.
  - Found while landing R1033 (2026-10-17).
  - Depends on R1031.
  - Model: Fable, Not Planned.

- [ ] **R1348 A struct operator and a multi-subscript are not calls in
  MIR, so `inline-parametric` keeps a `nodebug` dunder they reach**

  MIR lowering emits `a + b` on a struct as a `binary` naming its dunder
  and `g[i, j]` or `g[i:j]` as an `index.multi` or `slice` naming its
  accessor, where upstream's parser emits the dunder's call, as Mojito's
  `prefix_op` already does for `-x`.
  - Each pass that reads calls has to know these instructions as calls:
    `unused_args` and `unused_params` now do (R1110), but
    `Inliner::inlinable` reads a `call`, a resolved `call.method`, a
    one-operand `index.get`, and a devirtualized `call.indirect` only.
  - Probe: `via_op(a: V, b: V)` returning `a + b`, over an
    `@always_inline("nodebug") def __add__(self, o: V) -> Int`, keeps the
    `binary` after pre-elaboration, while `a.plus(b)` over the same body
    inlines.
  - Probe: `via_sub(a: V, i: Int)` returning `a[i, 2]`, over an
    `@always_inline("nodebug") def __getitem__(self, i: Int, j: Int)`,
    keeps the `index.multi`.
  - Lever: lower a struct operator to the dunder's `call.method`, as
    `prefix_op` does, and have the inliner bind a subscript's operands
    through its contract, as its `index.get` arm does.
  - Found while landing R1110 (2026-10-24).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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
  Iterable`, the inlined `for x in a` keeps `prepare:
  ["__trait_dispatch.__iter__$ov$SelfRead"]` and its `call.method` naming
  `__iterator_dispatch.__next__`, where upstream's substitution folds both
  witnesses.
  - The inliner's witness fold (`inline_parametric::fold_witnesses`) skips
    `GetIter` and that call: the iterator slot is typed by an associated
    projection (`T.IteratorType[o]`) only the elaborator solves.
  - `prepare` names the borrowed alternate
    (`symbol::borrowed_iterator_dispatch_alternate`) of `Iterable`'s
    `__iter__$ov$SelfRef` requirement, which no witness table records.
  - Fold both at once, through `StructDeclaration::witness_tables`, once
    the substitution closes `T.IteratorType[o]` at a concrete struct; the
    `abstract_value_next` rules in `mir/verify/calls.rs` then meet only
    an unfolded step.
  - Probe: `fc(xs: List[Int])` returning `count(xs)`, for
    `@always_inline("nodebug") def count[T: Iterable](a: T) -> Int`
    counting `for _ in a`; done when `fc`'s inlined body names
    `List.__iter__` and the iterator's `__next__`.
  - Since R890 the loop holds a `try` around its `__next__` call, so
    `count` keeps its call until R1288 lets a callee holding a `try`
    inline.
  - Found while landing R1052 (2026-10-22).
  - Depends on R1288.
  - Model: Fable, Not Planned.

- [ ] **R1281 The elaborator resolves a call through a bound by composing
  `{struct}.{method}`**

  A trait dispatch the elaborator meets at a concrete receiver is resolved
  by respelling the dispatch symbol under the receiver's struct name, where
  upstream's elaborator specializes the conformance's witness lookup.
  - The sites: `Specializer`'s call arm (`mono/specializer.rs`) through
    `symbol::resolve_method_symbol`/`retarget_method_symbol`, then
    `Instances::dispatched_overload_target`.
  - Read `StructDeclaration::witness` (the template's table) instead, and
    delete `dispatched_overload_target`; never compose the name (R1027).
  - Compiler-known traits other than `Equatable`/`Comparable` (`Hashable`,
    `Writable`, `Copyable.copy`) have no declaration, so their conformances
    record no table; their dispatches keep the composed name until the
    bundled library declares them.
  - The VM's retargeting of a surviving dispatch is R204's.
  - Probe: `assets/ok/trait_bound_comparison_drop_order.mojo` and
    `comparable_ordered_algorithms.mojo` print the same on both backends
    with `retarget_method_symbol` gone from `native::mono`.
  - Found while landing R1052 (2026-10-22).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R983 With the pre-elaboration pipeline off, a capturing callable
  parameter its body never calls fails to elaborate**

  `assets/ok/unread_callable_parameter_instances.mojo` fails under
  `MOJITO_PRE_ELABORATION=0` with "value parameter `f` is not compile-time
  constant" in `ignore_capturing`, so its `pre_elaboration::*` on/off trial
  fails.
  - With the pipeline on, `remove-unused-params` removes `f` and the
    elaborator binds no closure for it (R910), which hides the gap.
  - The elaborator should bind a capturing closure's environment for an
    uncalled parameter as it does for a called one.
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
    `MirSubscriptCall::target`, `GetIter::prepare`, and the checked
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

### Native Backend

Track: `native`.

The ABI-bump collector is last whatever else moves, because it batches every
change that needs a new `MJRT_ABI_VERSION`.

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

- [ ] **R852 Natively, `~` on a `UInt` is unsupported**

  Problem: `var u: UInt = 5; print(~u)` prints `18446744073709551610` on
  the VM and at the pin, while the native backend reports "unsupported
  operator `Invert` on `UInt` operand".
  - The VM's `apply_prefix` has a `UInt` arm, and Pliron's unary lowering
    has none.
  - Found while landing R846 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R433 Natively, a struct pack element's `write_to` takes the
  reflective default**

  Problem: `a[i].write_to(w)` over a pack element whose type declares its
  own `write_to` prints `P(1)` on the VM and at the pin, but `P(x=1)`
  natively.
  - Probe: `def f[*Ts: Writable](mut w: String, *a: *Ts)` writing each
    `a[i]` into `w`, called as `f(s, P(1), 2)`.
  - `"{}".format(P(1))` reaches the same call through the bundled
    `_FormatCurlyEntry._format`, so native `format` of such a struct is
    wrong too.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R434 Natively, a struct without its own `write_repr_to` cannot be
  formatted through a pack**

  Problem: `"{}".format(P(1))` over a struct declaring only `write_to`
  stops natively with "unsupported repr of `P` without compiled
  write_repr_to", where the VM and the pin print it.
  - `_FormatCurlyEntry._format` holds both `write_to` and `write_repr_to`
    calls per element, so every element type instantiates the repr branch.
  - The reflective default `write_repr_to` is synthesized in the checker
    (R424) and has no native body.
  - Probe: an `enum` declaring `Writable` with a case over a struct that
    declares `Writable` without `write_repr_to` stops natively at its
    generated `write_repr_to` ("unsupported repr of `Tracked` without
    compiled write_repr_to"), where the VM and the pin print it.
  - Found while landing R421 (2026-10-06).
  - Depends on R424.
  - Model: Opus, Not Planned.

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

- [ ] **R1424 Native `Writer.write` recognizes a `Named` argument by name**

  The `Writer.write` lowering (`lower/methods.rs`) formats a `Named` over a
  scalar inline by matching `$Named` in its struct name, where upstream
  calls `Named.write_to` like any struct's.
  - Without that arm, `text.write(Named("n", w))`
    (`conformance/fixtures/view_temporary_write.mojo`) stops natively with
    "unsupported display of a Pointer": no `Named.write_to` instance bound
    to the builtin string writer reaches the native program, so
    `append_struct_display` falls back to the reflective display of its
    `Pointer` field.
  - A `Named` over an aggregate, such as `Slice.write_repr_to`'s
    `Optional[Int]` bounds, already writes through its compiled `write_to`.
  - The lever: demand each struct argument's builtin-writer `write_to`
    instance at a native `Writer.write`, then delete the `Named` arm.
  - Found while landing R1158 (2026-10-25).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R633 Natively, a subscript method with a homogeneous variadic
  index is unsupported**

  Problem: `cube[1, 2, 3]` over `def __getitem__(self, *indices: Int)`
  prints `123` on the VM, but the native backend stops with "unsupported
  type `VariadicPack(Int)`" in `Cube.__getitem__`.
  - The `__setitem__` form (`*indices: Int, *, value: Int`) is the same
    shape.
  - Pinned on the VM by `vm_test`'s
    `multi_index_dispatch_supports_variadic_getitem` and
    `multidimensional_assignment_supports_variadic_setitem`.
  - Found while triaging the 2026-10-12 overnight gate.
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

### Catch Up To Current Mojo: Ownership, Origins, And Destruction

Track: `ownership`.

Every catch-up track closes a gap between Mojito and the pinned Mojo. Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

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
  - The slot of a value parameter holds no value: the elaborator replaces
    each read of it with the parameter's constant
    (`substitute_value_parameter_reads`), and materializes a constructed
    value through its constructor, so `lower_copies_and_moves` keeps such
    a `var.use copy` or `var.use move` a use (`materialized_parameter_slots`
    names the slots), as a lowered call would read storage the slot does
    not hold.
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

- [ ] **R1368 A value's abandonment is keyed by struct name or
  `TyKind::Param`, not by its type's `Deinitable` verdict**

  The checker marks a value linear by type shape, where upstream's
  `TypeDeclInfo::getDestructorForType` (`LowerLIT/CheckLifetimes.cpp`) asks
  each value's type.
  - Three switches handle a `TyKind::Param` or a struct only:
    `record_linear_temporary`, the `VarDecl` `linear_bindings` insert, and
    the parameter `linear_declarations` insert.
  - `explicit_destroy.rs` keys a struct by name and every type parameter as
    `LINEAR_TYPE_PARAMETER`, fixed at 'AnyType'.
  - Upstream refines a parameter's or projection's bound by the function's
    `where` facts. If every trait is linear, the message is the trait's own
    (`getMessageIfTraitIsLinear`).
  - Upstream judges a struct instance's `Deinitable` condition under those
    facts (`isTypeDeinitable`). Anything short of proven is linear, with the
    struct's message.
  - The lever: one verdict query that returns the denying message. The
    checker records it per binding, parameter, and temporary in
    `CheckedDeletability`, and the walk reads the recorded message.
  - The three switches, `LINEAR_TYPE_PARAMETER`, and the nominal
    `explicit_destroy_types` key go away.
  - Probe: R711's `return mk(a) < b` in `def g[T: Comparable & Copyable]`.
    The pin names 'Comparable', Mojito names 'AnyType'.
  - Closes R711 ('AnyType' named instead of the bound), R30 (an opaque `var`
    is not required to be `Deinitable`), and R667's temporary half. Shrinks
    R302.
  - R351 (a named `out` result called abandoned) is a different cause:
    upstream's `out` argument ends initialized.
  - Depends on R1364 (the three-valued verdict) and R1365 (the refined
    bound).
  - Model: Opus, Planned.

- [ ] **R1366 A `^` source is admitted per shape, not by upstream's origin
  rule**

  `check_transfer_source` (`checker/places.rs`) and the `Transfer` arm of
  `infer` (`checker/inference.rs`) admit `^` by special cases, where
  upstream's `UnaryOpNode::emitTransfer` (`ExprNodes.cpp`) applies one
  ordered rule.
  - The rule, in upstream's order:
    - An owned rvalue is passed through, with "no effect".
    - A trivial register type is copied, with "no effect".
    - Otherwise the source must reach a tracked root through field
      projections only (`OriginTrackable::findUnderlyingValueFromField`), or
      the error is "expression does not designate a value with an origin".
    - An immutable root is rejected: "cannot transfer out of immutable
      reference".
  - A subscript result, a `ref` binding, and a borrowed loop binder have no
    tracked root. Today `check_transfer_source` lets a `ref` binding
    through.
  - A capture is an indirect reference: "cannot consume indirect references
    to values" (`checkConsume` in `CheckLifetimes.cpp`).
  - Replace the per-shape tests: the `is_implicitly_copyable` indexed
    element and the `ref`-binding skip.
  - Keep the private `Tuple` storage element as a place until R341 lands.
  - Probe: `conformance/probes/reference_binding_transfer.mojo`. The pin
    says "does not designate a value with an origin". Mojito fails at run
    time with "vm: use of a value after it was moved".
  - Closes R31 (an implicitly copyable indexed element is transferred), R116
    (a `ref` binding transfer fails late), and R120 (a `mut` capture is
    transferred and refilled).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1367 MIR lowering re-decides whether a transfer copies or moves**

  MIR `transfer` (`mir/lower_expr/expr.rs`) and the temporary binders in
  `mir/calls.rs` choose copy or move by `owns_droppable_storage`, where
  every struct counts as owning, while upstream decides once, in
  `emitTransfer`, from the type's triviality.
  - The lever: R1366 records its verdict as a node fact (pass-through,
    trivial copy, or place move), and lowering reads it.
  - The temporary binders read the same fact, so a register-passable struct
    stays in a register.
  - Probe: `conformance/probes/trivial_struct_mut_parameter_transfer.mojo`.
    The pin prints `3 3`. Mojito says "'v' is uninitialized at return from
    this function".
  - Closes R121 (a trivial struct transferred out of a `mut` parameter).
    Shrinks R750 (a field transfer lowered as a move before the discard fact
    is read).
  - Depends on R1366, which records the verdict.
  - Model: Opus, Not Planned.

- [ ] **R30 A `var` of an opaque type is not required to be `Deinitable`**

  Problem: `var v: FT = FT()` under `comptime if conforms_to(FT, Defaultable
  & Writable):` validates at Mojito, while the pin reports "'v' abandoned
  without being explicitly destroyed ... consider adding trait conformance
  to Deinitable" until `Deinitable` is proved too.
  - Mojito accepts a program the pin rejects, so this is a divergence; it
    holds for every opaque parameter, not only a reflected field type.
  - The abstract destruction walk (`explicit_destroy.rs`) does not ask the
    view's bounds for `Deinitable`.
  - Probe: `var y = Ts[0]()` then `print(y)` in `def f[*Ts: Movable &
    Defaultable & Writable]()`, also spelled `var x:
    TypeList[Ts.values].__getitem_param__[0]`; the pin reports "'y'
    abandoned without being explicitly destroyed" (found while landing
    R1168, 2026-10-20).
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

- [ ] **R1221 A `ref` binding over an owned subscript result fails MIR
  verification**

  Problem: `ref r = ns[0]`, over a `__getitem__` that returns an owned
  `String`, fails with "dynamic element projection requires checked
  indexed storage, got Names"; the pin binds the materialized result and
  prints `n0`.
  - The `RefDecl` arm (`checker/statements.rs`) treats a syntactic index
    as a place, rather than materializing the call's owned result as
    `materialized_reference_actual` does for other calls.
  - Probe: `conformance/probes/ref_binding_owned_subscript_result.mojo`.
  - Probe: the same over an `Int` result (`ref q = t[1]`) fails the same
    way, where the pin rejects it with "value of type 'Int' doesn't have a
    memory origin in 'ref' binding", the rule R1145 brings to the plain
    `ref` declaration.
  - A sequence pattern's owned element read avoids it by reading into a
    hidden `var`.
  - Found while planning R1135 (2026-10-20).
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
  - The checker already records a call result's register passability
    for MIR (`IndirectResult`, `Checker::record_result_conventions`); a
    transfer could carry the same fact.
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
  - The pin reports the unproven clause as "lacking evidence", which needs
    R1364's three-valued query.
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
  - The checker's acceptance of that residual destruction is R1368's.
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
  - Related: R1221 (a `ref` binding over an owned subscript result).
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

- [ ] **R711 An abandoned type-parameter temporary's diagnostic names
  `AnyType`, not its bound**

  Problem: `return mk(a) < b` in `def g[T: Comparable & Copyable](a: T,
  b: T)` is rejected by both, but the pin says `unhandled explicitly
  destroyed type 'Comparable'` and Mojito says `'AnyType'`.
  - The same holds for `h(mk(a))` at a read parameter.
  - Cause: `run_explicit_destroy` (`crates/mojito-checker/src/checker.rs`)
    keys every linear type-parameter temporary as
    `explicit_destroy::LINEAR_TYPE_PARAMETER`, fixed at `'AnyType'`.
  - `demand_chain_middle_destroyable` already names the first bound, as
    the pin does.
  - Found while landing R709 (2026-10-13).
  - Depends on nothing.
  - Model: Fable, Not Planned.

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
  - Found while planning R1166 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1373 A subscript store selects `__setitem__` without typing the
  target through `__getitem__`**

  `check_nominal_subscript_assignment` (`checker/indexing.rs`) picks the
  setter from the right-hand side alone, and the `AugAssign` arm
  (`checker/statements.rs`) runs its own getter-then-setter selection, where
  upstream's `emitGetterSetterAccess` (`ExprNodes.cpp`) builds one
  `SubscriptDLValue` for every store.
  - Upstream requires a getter ("'T' has '__setitem__' but no '__getitem__'
    method"), selects it over the indices, takes its result type as the
    element type, and stores a value of that type through the setter.
  - The lever: one checker subscript-lvalue function that both statement
    arms call.
  - Fixtures that declare a setter alone need a getter added.
  - Probe: `s[1] = 4.0`, with `__getitem__(self, a: Int) -> Int` and
    `__setitem__(mut self, a: Int, value: Float64)`, prints `4` in Mojito.
    The pin says "value passed to 'value' cannot be converted from 'Int' to
    'Float64'".
  - Closes R35 (a store with a setter but no getter is accepted) and R690
    (the getter's arity is not checked on a store).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1374 A destructuring leaf stores through its own path, not the
  assignment's lvalue**

  `destructure_into_place` (`checker/destructure.rs`) rejects a nominal
  subscript leaf, because the extraction plan has no setter store, where
  upstream's `TupleDLValue::emitStore` (`DLValues.cpp`) stores each element
  through `emitStoreToLValue`, so a subscript leaf goes through its
  `SubscriptDLValue`.
  - The lever: the leaf takes R1373's subscript lvalue. MIR gains a setter
    call whose value is an element register (`lower_subscript_set` takes
    only an expression today).
  - Probe: R1247's `(xs[0], xs[1]) = pair`, which the pin runs, printing `3
    4`.
  - Closes R1247 (a nominal subscript as a destructuring target). Shrinks
    R1224 (the order of a nested sequence store).
  - Depends on R1373, which supplies the lvalue.
  - Model: Opus, Not Planned.

- [ ] **R1375 An assignment target must match a list of shapes in the
  parser**

  `parse_expr_or_assign` (`parser/stmts.rs`) accepts only listed kinds as
  plain and augmented targets, and the walrus accepts only a name
  (`parser/exprs.rs`), where upstream parses an assignment over any
  expression and rejects a non-lvalue in `IREmitter::emitLValue` with
  "expression must be mutable", using `emitExprLValue` for `=`, `+=`, and
  `:=` alike.
  - The lever: the parser takes any expression as a target.
    `check_place_impl` gains an `Invoke` arm, and the walrus stores through
    the same lvalue.
  - Update `docs/grammar.md` first (`assign_target`, `NAME ':='`).
  - Probe: `print(xs[0] := 5)` then `print(xs[0])` prints `5` twice at the
    pin. Mojito stops parsing at "walrus ':=' target must be a name". Also
    `conformance/probes/invoke_assignment_target.mojo`.
  - Closes R1233 (a call with explicit parameters as a target).
  - Depends on R1373, so a walrus over a setter stores as `=` does.
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
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
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

- [ ] **R1233 A call with explicit parameters is not an assignment target**

  Problem: `w.at[0]() = 5` and `w.at[0]() += 1`, over `def at[k: Int](mut
  self) -> ref[self.x] Int`, are parse errors ("invalid assignment target:
  Invoke"), where the pin prints `6`.
  - The same call without a bracket (`w.at() = 5`) runs.
  - `Parser::parse_expr_or_assign`'s plain and augmented target lists
    (`crates/mojito-parser/src/parser/stmts.rs`) omit `ExprKind::Invoke`.
  - With the parser admitting it, the checker's place check
    (`checker/indexing.rs`) has no `Invoke` arm and rejects it the same way.
  - Probe: `conformance/probes/invoke_assignment_target.mojo`.
  - Found while landing R1132 (2026-10-20).
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

- [ ] **R1238 A leading-dot operand of `==` has no contextual type**

  Problem: `d == .int64` over a `DType`, and `case .int64:` over one, fail
  with "no contextual type is available here", where the pin prints
  `True` and runs the case.
  - Upstream types the operand from the selected `__eq__`'s parameter; a
    leading-dot case head over a subject that is not `EnumLike` is the
    same `==` test.
  - Mojito resolves a leading dot only against an annotation or an
    expected parameter type (`with_contextual_root`), never an operator's
    other operand.
  - Probe: `conformance/probes/leading_dot_equality_operand.mojo`.
  - Found while landing R1133 (2026-10-20).
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

- [ ] **R1247 A nominal subscript is not a destructuring target**

  Problem: `(xs[0], xs[1]) = pair` is rejected ("a nominal subscript
  (`xs[i]`) as a destructuring target"), where the pin stores each element
  through `__setitem__` and prints `3 4`.
  - `checker/destructure.rs` rejects the leaf because the extraction
    plan's element store has no `__setitem__` path.
  - The plain `xs[i] = v` store has one (`lower_subscript_set`).
  - Fixture to add beside `assets/ok/destructure_swap_and_places.mojo`.
  - Found while landing R1140 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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
  - The synthesized `__init__` must be the member the struct's
    `Defaultable` witness table records (`compiler_trait_witness_table`
    in `checker/traits.rs`), since `T()` at a struct names that witness
    (`MirDeclarations::default_initializer`).
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

- [ ] **R1357 A struct operator is checked apart from a method call**

  The checker types `a op b` and `-x` over a struct through its own
  selection (`struct_infix_dispatch`, `struct_reflected_dispatch`,
  `struct_dunder_signature_for` in `checker/operators.rs`), where upstream's
  `emitBinOpCall` and `UnaryOpNode::emitArith` (`MojoParser/ExprNodes.cpp`)
  resolve `a.__op__(b)`, then `b.__rop__(a)`, through
  `OverloadSet::lookupAndResolve` as an ordinary method call.
  - The lever: check a struct operator as `infer_method_call` over the
    dunder, so it records the whole `SelectedCall` contract, its raise
    effect, and the right operand typed by the selected parameter, as an
    argument is.
  - Keep the `overload_target` fact, derived from the contract, until R1348
    lowers the call from it.
  - `NegatedEquality` stays until R705 bundles `Equatable.__ne__`'s default.
  - Probe: `l == [1]` over `l: List[Int]` (R697) and `print(-(n^))` over
    `__neg__(deinit self)` (R1332) run at the pin and fail in Mojito.
  - Closes R1332 (a prefix dunder records no call contract) and R697 (a
    display right operand is not typed by the dunder). Shrinks R1348 and
    R1238.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1332 A struct prefix operator records no call contract, so a
  `deinit self` dunder is rejected**

  Problem: `print(-(n^))` over `def __neg__(deinit self) -> Int` stops
  with "invalid checked program: … method-call receiver convention does not
  match 'N.__neg__'", where the pin prints `-3`.
  - `infer_prefix` (`checker/operators.rs`) records only the selected
    dunder (`ResolveCallable`), where an ordinary method call records its
    whole contract (`SelectedCall`) and upstream checks `-x` as
    `x.__neg__()`.
  - MIR's `prefix_op` (`mir/lower_expr/expr.rs`) therefore reads the
    receiver convention off the operand's `ForwardedArgument` mark, which
    says only that the receiver is owned, so it records `var`.
  - Lever: record the selected call's contract for a struct prefix
    operator, and lower its `call.method` from it.
  - Found while landing R1325 (2026-10-24).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1364 A conformance query answers yes or no, never "lacking
  evidence"**

  `conforms_to` and the `is_copyable`/`is_movable`/`is_deinitable` family
  (`checker/traits.rs`) answer a Bool with per-trait defaults, where
  upstream's `ASTType::doesConformTo` returns a `TriBool` checked against
  the scope's known assumptions
  (`ASTDecl::getKnownAssumptionsIncludingParents`,
  `canDischargeConstraintsInScope` in `MojoParser/Constraints.cpp`).
  - The lever: one query returning proven, refuted, or unknown. It reads the
    declared bounds, `projection_bounds`, and one stack of assumed facts.
    Bool callers keep asking for proven.
  - Merge `assumed_conformances` and `assumed_propositions` into one
    per-scope set, as upstream keeps one `knownAssumptions` list.
  - Delete the `leaf_conforms(..).unwrap_or(true)` fallbacks. An associated
    projection is unknown unless its bounds prove the trait.
  - `constraint_proposition`'s `Conforms` arm (`constraints.rs`) maps
    unknown to a hole, so a symbolic parameter reports "lacking evidence". A
    `comptime if` arm keeps folding.
  - Probe: R698's `var d = {1: x.copy()}` in `def f[T: Copyable & Hashable &
    Equatable](x: T)`. The pin says "lacking evidence to prove correctness",
    Mojito says "violated constraint".
  - Closes R698 (a missing conformance is reported as violated, not
    unproven). Shrinks R667 (its `is_deinitable` half) and R129 (the pin
    rejects it when checking the conformance).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1365 A type parameter's bound is not refined by its scope's `where`
  facts**

  Method lookup over a binder widens its bounds by an assumed conformance
  only for `copy` and `__hash__` (`resolve_bound_method`,
  `method_calls/resolution.rs`), where upstream's `maybeRefineParamType`
  (`ExprNodes.cpp`) merges the declared bound with
  `getTraitBoundFromAssumptions` into one refined type every consumer sees.
  - The lever: one effective bound per binder or projection, made of the
    declared bounds plus every trait R1364's evidence set proves.
  - Method lookup asks for that bound, and the `copy` and `__hash__` cases
    go.
  - Record the refined bound on the selected call's requirement, so
    `MirDeclarations::bound_requirement` finds it.
  - `CheckLifetimes` refines the same way
    (`refineWithContextualWhereClauses`), which R1368 reuses.
  - Probe: R704's `def run[T: Copyable](s: T) where conforms_to(T, Sh)`
    calling `s.sh(31)`. The pin runs it, Mojito says "type 'T' has no method
    'sh'".
  - Closes R704 (a requirement under a `where conforms_to` clause is
    rejected). Shrinks R30.
  - Depends on R1364, which supplies the evidence set.
  - Model: Fable, Planned.

- [ ] **R704 A requirement called under a `where conforms_to(T, Trait)`
  clause is rejected**

  Problem: `def run[T: Copyable](s: T) where conforms_to(T, Sh)` calling
  `s.sh(b.t)` prints `del 31`, `sh` at the pin, but Mojito rejects it:
  "type 'T' has no method 'sh'".
  - The method lookup over a binder (`method_calls/resolution.rs`) widens
    the receiver's bounds by an assumed conformance only for `copy` and
    `__hash__`; it should take every trait the clause assumes.
  - The call's receiver type then still names only `Copyable`, so
    `ArgumentPassing` (`register_loans.rs`) finds no requirement through
    `MirDeclarations::bound_requirement` and keeps `b` through the call.
    The fix must give the drop analysis the assumed trait too.
  - Found while landing R682 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R707 A struct declaring `Comparable` does not conform to
  `Equatable`**

  Problem: `eq(S(9))` over `def eq[T: Equatable & Copyable](a: T)`, where
  `S` declares `(Comparable, Copyable)`, is rejected with "type 'S' for
  parameter 'T' does not conform to trait 'Equatable'", where the pin
  accepts it because `Comparable` refines `Equatable`.
  - `struct_conformance_applies` asks `trait_refines`, which knows no
    refinement between compiler-known traits.
  - `compiler_trait_declaration` (`mojito-checked::declaration`) already
    records `Comparable` refining `Equatable`.
  - Probe: `p.__lt__(q)` over `p = (Tag(2), 1)`, where `Tag` declares
    `Comparable`, is rejected because `Tuple.__lt__`'s `where` clause
    `conforms_to(Ts.values, Equatable)` folds False. The pin prints `True`.
  - Found while landing R702 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R708 A dunder call through a compiler-known comparison bound is
  rejected**

  Problem: `a.__lt__(b)` in `def g[T: Comparable](a: T, b: T)` is
  rejected with "type 'T' has no method '__lt__'", where the pin runs it.
  - Method resolution over a bound reads only the declared traits
    (`self.traits`), and `Equatable`/`Comparable` are compiler-known.
  - `compiler_trait_declaration` holds the requirement signatures, and an
    operator over the bound already names the same
    `__trait_dispatch.__lt__$ov$Self` target.
  - Found while landing R702 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R706 A struct declaring only `__lt__` has no `>`, `<=`, or `>=`**

  Problem: over a `struct S(Comparable)` declaring `__lt__` and `__eq__`,
  `S(2) >= S(1)` is rejected with "operator '>=' is not defined", and
  `a >= b` through `T: Comparable` stops in the VM with "unknown method
  'S.__ge__'", where the pin prints `True` both times.
  - Upstream's `Comparable` defaults `__gt__`, `__le__`, and `__ge__`
    through `__lt__`; Mojito has no declaration to take them from.
  - Found while landing R702 (2026-10-13).
  - Depends on R705.
  - Model: Opus, Not Planned.

- [ ] **R1358 An operator over a bound type parameter is not a call of the
  bound's requirement**

  `a + b` or `a < b` over `T: Trait` is typed by the operator's trait name
  (`param_has_bound` with `infix_operation_trait` and
  `prefix_operation_trait`, `has_order_bound`, `bound_comparison_target`),
  where upstream resolves `a.__add__(b)` against the requirements the
  bound's traits declare, as any method call through a bound.
  - The lever: route the bound case through R1357's method-call path. A user
    trait declaring `__add__` then works, and the comparison and `Powable`
    special arms go.
  - Probe: `add2[T: Addy](a, b)` returning `a + b` prints `7` at the pin,
    where Mojito says "operator '+' is not defined for T and T".
  - Closes R1282 (an arithmetic operator through a user trait bound is
    rejected).
  - Depends on R1357 (the operator is a method call) and R705 (comparisons
    find declared requirements).
  - Model: Opus, Not Planned.

- [ ] **R690 A subscript assignment is accepted without a `__getitem__`
  matching its indices**

  Problem: `s[k] = v` on a struct whose `__setitem__(mut self, a: Int,
  b: Int)` sits beside `__getitem__(self, a: Int, b: Int)` runs in Mojito,
  while the pin rejects it: "invalid call to '__getitem__': missing
  required argument: 'b'".
  - The pin type-checks a subscript assignment's target through
    `__getitem__` over the same indices before it selects the setter; the
    checker's subscript-store selection checks the setter alone.
  - R35 is the getter-absent half of the same check.
  - Found while landing R681 (2026-10-13).
  - Depends on R35.
  - Model: Opus, Not Planned.

- [ ] **R667 An associated projection is treated as `Deinitable` and
  `Movable` whatever its bounds**

  Problem: `def g[T: Source](mut s: T)` doing `_ = s.get()`, where
  `Source` declares `comptime Element: Movable` and `def get(mut self) ->
  Self.Element`, runs, but the pin rejects it: "'(expression temporary)'
  abandoned without being explicitly destroyed".
  - Upstream's own `_DropIterator` asserts `conforms_to(Self.Element,
    Deinitable)` before it discards an `Iterator.Element`, which is bound
    only by `Movable`.
  - The cause: `is_deinitable` and `is_movable`
    (`crates/mojito-checker/src/checker/traits.rs`) fall through to
    `leaf_conforms(..).unwrap_or(true)` for every `Assoc`.
  - The lever: R1364's three-valued query answers an `Assoc` from
    `projection_bounds`, as `assoc_member_bound_proves` already does for
    `Copyable`.
  - The same discard of `it.__next__()` over `T: Iterator` is rejected
    first by R1152: `__next__` raises `StopIteration` into a plain
    `raises` context.
  - Expect fallout in generic bundled-std bodies that drop a projected
    value.
  - `moving_an_iterable_while_its_generic_iterator_lives_rejects`
    (`tests/vm_test.rs`) discards an element. If it starts failing for
    this reason, give it a non-discarding read.
  - Found while landing R665 (2026-10-13).
  - Depends on R1364.
  - Model: Opus, Not Planned.

- [ ] **R792 `conforms_to` takes only a parameter name and one trait**

  Problem: `where conforms_to(IteratorType.Element, Movable & Deinitable)`
  is rejected with "conforms_to requires a parameter name", where the pin
  accepts a projection subject and a trait composition.
  - The bundled `std/builtin/_stubs.mojo` omits upstream's clause on
    `paramfor_has_next` and `paramfor_next_iter` for this reason. Restore
    it when this lands.
  - The constraint compiler is `compile_generic_constraint`
    (`crates/mojito-checker/src/checker/constraints.rs`).
  - Depends on R667, which first gives an associated projection its real
    bounds.
  - Model: Opus, Not Planned.

- [ ] **R858 A program's own `def Int` breaks a library body's builtin
  `Int(...)`**

  Problem: `def Int(x: Bool) -> Bool: return x` beside `print(Int(True))`
  prints `True` at the pin and stops in Mojito with "in
  `__module$std$$intrinsics$_uint_digits_at`: unsupported monomorphizing
  `Int` return: expected `Bool`, found `Int`".
  - The library's bare `call Int` resolves to the program's declaration
    once both are linked, where at the pin the user's `Int` shadows the
    builtin in its own module alone.
  - The same holds with `MOJITO_PRE_ELABORATION=0`, so `sccp` is not the
    cause; its conversion fold already declines a declared name.
  - Probe: `def Int(x: Float64) -> Float64: return x + 1` and
    `print(Int(Float64(2.0)))`, which prints `3.0` at the pin.
  - Found while landing R856 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R697 A display as a struct operator's right operand is not typed
  by the dunder's parameter**

  Problem: `l + [2]` and `l == [1]` over a `List[Int]` `l` are rejected
  ("expected List[Int], found Array[Int, 1]"), where the pin runs them.
  - The checker types the right operand's display by its natural `Array`
    instead of by the selected dunder's parameter, as a call argument's
    display is.
  - The pin builds that display at the operator's setup: `g(a) + [a.t]`,
    where `g` adds 100 to `a.t`, reads `a.t` after `g` runs.
  - Once typed, `lower_operator_operand`
    (`crates/mojito-mir/src/mir/calls.rs`) must pass `bound = true` for the
    right operand; the left operand is a receiver and keeps its natural
    type (`[a.t] == g(a)` is rejected at the pin too).
  - Found while landing R693 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1359 Tuple comparison is decided structurally, not by `Tuple`'s
  dunders**

  The checker accepts `p < q` and `p == q` over tuples by
  `tuple_order_compatible` and `tuple_elements_equatable_nominal`
  (`checker/builtins.rs`, `operators.rs`), where upstream calls the bundled
  `Tuple.__lt__` and `__eq__` under their `where conforms_to` clauses.
  - Delete both structural arms. A tuple operand then dispatches as R1357's
    struct operator does.
  - Probe: `(Tag(2), 1) < (Tag(2), 3)` over a user `Comparable` `Tag` prints
    `True` at the pin. Mojito rejects it.
  - Mixed built-in elements, such as `(1, String("b")) < ...` and
    `(2.0, String("x")) < ...`, already run.
  - `Tuple.__lt__` itself rejects `Tag` until R707 makes a `Comparable`
    struct `Equatable`.
  - Depends on R1357 and R707.
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
    side the rename needs exists; `Hasher.update` itself is still the
    checker's intrinsic, not a real `SIMD.__hash__` body.
  - Upstream also replaced the pointer-and-length `hash()` overload with
    `hash_bytes(ImmSpan[Byte])`, which lands in the same pass.
  - The levers are `checker/traits.rs` (the `Hasher` requirement set and its
    shape message), `checker/method_calls/intrinsic_receivers.rs` (the intrinsic arms),
    and `stdlib/std/hash/` (`hasher.mojo`, `_ahash.mojo`, `_fnv1a.mojo`
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

- [ ] **R479 A partial explicit application does not infer the remaining
  parameters**

  Problem: `g2[m: Int, n: Int](a: Box[n])` called as `g2[1](Box[4](0))` is
  rejected with "cannot infer type parameter 'n' of 'g2' from the
  arguments", where the pin prints `5`.
  - The same holds for a lane: `h[DType.int8](Box[4](0), Int8(1))` for
    `h[dt: DType, n: Int](a: Box[n], b: Scalar[dt])`, pin `4`.
  - It holds for a keyword-only prefix too: `k[s="yo2"](Int16(2))` for
    `k[dt: DType, s: String](x: Scalar[dt])`, and `z[T=P](Int8(1))`.
  - Once any parameter is spelled, the checker solves none of the rest from
    the arguments.
  - Found while landing R477 (2026-10-07).
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

- [ ] **R382 An `Int` binder is not inferred from a vector width spelled by
  an `Int` expression**

  Problem: `show[w: Int](x: SIMD[DType.int32, w])` called as `show(a)` with
  `a = SIMD[DType.int32, n](1)` in `def f[n: Int]()` is rejected with
  "cannot infer type parameter 'w' of 'show' from the arguments", where the
  pin binds `w` to `n` and prints.
  - `solve_value_args` (`checker/generics.rs`) solves a vector's width slot
    only for a binder declared `SIMDLength`.
  - That is right for a width that is a literal, which the pin leaves an
    `Int` binder unresolved against.
  - A width spelled by an `Int` expression (`n`, an element of a local
    `comptime` display) converts the same way on both sides, so the pin
    solves the binder.
  - Found while landing a display binding read in a type or a parameter
    argument (2026-10-06).
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
  - Probe: `var f = S.st` over `@staticmethod def st(x: Int) -> Int` fails
    with "Undefined variable 'S'", where the pin prints `f(3)` (found
    while landing R1032, 2026-10-17).
  - Probe: a value member read through a type member alias at run time,
    `print(S.B.w)` over `comptime B = Box[3]`, fails with "Undefined
    variable 'S'", where the pin prints `30`; the local `comptime W =
    S.B.w` works.
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
  - Probe: a generic module alias's construction, `W[Int](8)` over
    `comptime W[T: AnyType] = Pt[T]`, fails with "Undefined variable 'W'",
    where the pin constructs `Pt[Int]`.
  - Probe: over a pack, `T2[Int, Bool](1, True)` for `comptime T2[*Ts:
    Copyable] = Tuple[*Ts]`, and `comptime for v in V[1, 2, 3]()` for
    `comptime V[*vs: Int] = ParameterList[vs.values]`, fail with
    "Undefined variable", where the pin prints `True` and `1 2 3`.
  - Probe: a local `comptime a = A[4]()` over `comptime A[k: Int] = W[k]`
    fails with "a compile-time call of 'A' is not evaluated above the
    check", where the pin constructs `W[4]`.
  - Probe: a run-time construction through a struct's member alias, `var t
    = TypeList.of[Trait=AnyType, Int, Bool]()` then `print(len(t))`, fails
    with "Undefined variable 'TypeList'", where the pin prints `2`. The
    elaborator expands it to `TypeList[Trait=..., values=TypeList.of[...]
    .values]()` (`Elab::type_construction`), whose closed type list
    `resolve_param_arg` does not record as an attribute: recorded, MIR
    emits it as an untyped constant.
  - Probe: a construction through a non-generic struct's member alias,
    `S.V(1)` over `comptime V = SIMD[DType.int64, Self.w]`, fails with
    "Undefined variable 'S'", where the pin prints `[1, 1, 1, 1]`.
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

- [ ] **R684 An explicit dunder call on a scalar is rejected**

  Problem: `var q = 5; print(q.__add__(2))` prints `7` at the pin, but
  Mojito rejects it: "type 'Int' has no method '__add__'".
  - The operator spelling `q + 2` runs; only the explicit method call on
    a compiler-known scalar has no member to resolve.
  - Found while landing R675 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R1282 An arithmetic operator through a user trait bound is
  rejected**

  Problem: `def add2[T: Addy](a: T, b: T) -> T: return a + b`, for `trait
  Addy` declaring `def __add__(self, other: Self) -> Self`, fails with
  "operator '+' is not defined for T and T", where the pin prints `7` for
  `add2(Box[1](3), Box[1](4)).x`.
  - The checker dispatches an operator through a bound only for a
    comparison (`operators.rs::bound_comparison_target`, `Equatable` and
    `Comparable`); any other dunder a bound's trait declares is not looked
    up.
  - Select the requirement the bound's trait declares for the operator's
    dunder, as a method call through a bound does, and record its
    dispatch symbol as the `binary`'s `resolved`; the inliner's witness
    fold then folds it as it folds a comparison.
  - Found while planning R1052 (2026-10-22).
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

- [ ] **R342 An initializer list with keyword entries, or bound by a
  `comptime` statement, is rejected**

  Problem: `take({1, kw=2})` and `comptime e: Tuple[Int, Int] = {}` are
  valid Mojo, the initializer list being the construction `T(1, kw=2)`,
  where Mojito's parser has no keyword entry in `brace_literal` and the
  elaborator reports `an empty '{}' display needs a Dict[K, V] type
  annotation` for the binding.
  - `{}` and `{a, b}` at a runtime contextual type landed on 2026-10-05
    (`checker/initializer_list.rs`).
  - The parser's `brace_literal` and `comptime/elab.rs` are the two sites.
  - The bundled `List.__getitem__(ContiguousSlice)` spells
    `Span[...](unsafe_ptr=..., length=...)` where upstream returns
    `{unsafe_ptr = ..., length = ...}`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R495 An initializer list at a struct type over a parameter is
  rejected**

  Problem: `return {0}` in a method returning `It[Self.T]` fails with
  "cannot emit initializer list for 'It[T]': the type has no construction
  to spell", where the pin constructs `It[Self.T](0)`.
  - `checker/initializer_list.rs` spells the construction through
    `ct::source_type`, which has no spelling for a type parameter or a
    value-pack argument.
  - The bundled `ParameterList.__iter__` spells
    `_ParameterListIter[*ParameterList[Self.values]()](0)` where upstream
    writes `{0}`.
  - Probe: `var x: TypeList.of[Trait=AnyType, Int, Bool] = {}` and, in
    `def f[*Ts: AnyType]`, `var x: TypeList[Trait=AnyType, Ts.values] =
    {}` fail with "cannot emit initializer list for 'TypeList[AnyType,
    ...]'", where the pin prints `len(x)`: a raw-list argument has no
    source spelling either.
  - Probe: the bundled `VariadicList.__iter__` spells
    `_VariadicListIter[...](0, self)` where upstream writes `{0, self}`.
  - Found while landing R325 (2026-10-08).
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

- [ ] **R419 A trait method cannot declare a `*` or `/` marker**

  Problem: `def f(self, *, k: Int) -> Int` in a trait body fails with
  "positional-only/keyword-only markers on trait methods", where the pin
  accepts it and dispatches `t.f(k=4)` through a bound.
  - The rejection is in `Checker::check_trait` (`checker/traits.rs`).
  - The requirement's keyword-only boundary is already built by the rule a
    struct method uses (`Collectors::of`), so a `*args` collector's
    boundary is modeled; the explicit markers are not.
  - Found while landing R413 (2026-10-06).
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

- [ ] **R698 A constraint on a type parameter that lacks the trait is
  "violated" where the pin says "lacking evidence"**

  Problem: `var d = {1: x.copy()}` in `def f[T: Copyable & Hashable &
  Equatable](x: T)` is rejected by both, but Mojito says "violated
  constraint" and the pin says "lacking evidence to prove correctness".
  - `constraint_verdict`'s `Conforms` arm
    (`crates/mojito-checker/src/checker/constraints.rs`) folds a symbolic
    parameter's missing conformance to false; the pin leaves it unproven.
  - The same holds for the spelled
    `Dict[Int, T](keys=[1], values=[x.copy()], __dict_literal__=None)`.
  - Keep a `comptime if conforms_to(T, ...)` arm's folding as it is.
  - Found while landing R694 (2026-10-13).
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

- [ ] **R1400 Eighteen `comptime` entries are fixed or name code that no
  longer exists**

  The track overstates what remains: six entries were fixed by later work,
  and eleven cite functions or diagnostics that are gone from `crates/`, so
  their causes cannot guide a session.
  - Fixed, by probes run 10-25 on Mojito and the pin: R279 (its conformance
    probe prints `3 30 1`), R337 (`g[fib(6)]` and `W[fib(6)]` run), R340,
    R543, R98, and R278.
  - Partly fixed: R72's call-bracket form `size[Self.n * 2]()` runs. Only
    the alias body remains, and R1371 closes it.
  - Cite `Elab::unroll_comptime_for` or `Elab::defer_constant`, both gone:
    R319, R368 (`top_consts` still exists, so it is probably live), and
    R509.
  - Cite R7's fix and its gone diagnostics ("unspecialized type-keyed
    method", "unknown compile-time function",
    `is_specializable_declaration_in`): R131, R132, and R486.
  - Cite other gone code: R82 (`resolves_top_template`), R464
    (`check_frozen_structs`), R465 (`ctfe_struct_entry`, "is not a
    compile-time-callable"), R450 (`inferred_param_arg_regs`), and R498
    ("callable contract names parameter").
  - For each entry, run its probe (or the program its first sentence names)
    on `target/debug/mojito` and on the pinned Mojo, through the Pixi
    project `docs/usage.md` names (`MOJO_PIXI_MANIFEST`).
  - Delete an entry whose probe now matches the pin, and record it in
    `CHANGELOG.md`.
  - Rewrite an entry that still fails so its cause names today's code, and
    keep its ID.
  - Merge a duplicate into the entry it restates, carrying its probes, and
    delete it.
  - Done when every entry listed here is deleted, rewritten against today's
    code, or merged.
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

- [ ] **R1369 A type expression resolves by its spelling, not by what its
  names denote**

  The checker builds a type from a `Type` spelling (`ty_from_anno`) and
  turns only a lone identifier from a value argument into a type
  (`resolve_param_arg`), where upstream parses every annotation as an
  expression and resolves it by what it denotes (`parseType` in
  `DeclResolution.cpp`, then `IREmitter::emitExprType`).
  - The lever: one checker resolver that types an expression as a type
    through scope lookup.
    - A struct or alias gives its type, and a subscript of a type gives its
      application.
    - A local `comptime` value with `__getitem_param__` gives its element.
    - `type_of(value)` gives the value's type.
  - Sites: `ty_from_anno` (`Type::Named`, `Type::TypeOf`) and
    `resolve_param_arg` (`checker/type_resolution.rs`), and the receiver
    reinterpretation in `method_calls/type_receivers.rs`.
  - Probe: `var y: type_of(x) = 3` prints `3` at the pin. Mojito says
    "'type_of' of a value ... not supported yet".
  - Closes R134 (`type_of` of a value as a type) and R1203 (a `comptime`
    `TypeList` name subscripted in an annotation).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1370 A bracket argument is a type or a value by its
  capitalization**

  `parse_param_arg` parses a capitalized `Name[` as `ParamArg::Type`, and a
  fixed name list (`peek_starts_type`) as a type, where upstream parses
  every bracket operand as one expression
  (`ExprParser::parseSubscriptSuffix`, then `parseExprOrSlice`) and lets
  sema classify it.
  - The lever: parse every name-led operand as an expression. Only spellings
    that just the type grammar can write stay `ParamArg::Type`: `def(...)`
    types, `ref`, and `*Ts`.
  - R1369's resolver turns the expression into a type wherever a type
    parameter takes it.
  - Pre-check consumers (`mojito-comptime`'s `params.rs` and `eval.rs`,
    `simd_width.rs`) read a value operand's type spelling where they read
    `ParamArg::Type` today.
  - Update `docs/grammar.md` first (`param_arg`, the lowercase-name notes).
  - Probe: `comptime L = [1, 2]` then `print(materialize[L[1]]())` prints
    `2` at the pin. Mojito says "takes one compile-time value".
  - Closes R377 (a capitalized `L[1]` in `materialize[...]`), R381 (`L[0] *
    2` stops the parser), the capitalized half of R512, and the
    `s[Int16(5)]()` half of R482.
  - Depends on R1369, which supplies the resolver.
  - Model: Opus, Planned.

- [ ] **R1371 A bracket after an expression is an index, an application, or
  a dereference by its shape**

  `parse_bracket_suffix` picks `Index`, `MultiIndex`, `TypeApply`, or an
  `EmptySubscript` dereference from the base's capitalization and the
  operand shape, where upstream builds one `SubscriptNode` and
  `SubscriptNode::emitLCVIR` (`ExprNodes.cpp`) classifies it by what the
  base denotes.
  - Upstream: an overload set binds parameters, a type is applied, and a
    value calls `__getitem_param__` or `__getitem__`.
  - The lever: the parser always emits `Index` or `MultiIndex`, empty
    brackets included. The checker's subscript arms route a base that
    denotes a type or a generic function to the existing application check.
  - The `reflect`, `origin_of`, spread, and `None` special cases in
    `suffix.rs` go away.
  - Merging the four AST kinds into one node can follow in `mojo-shape`.
  - Probe: `var f = twice[4]` over `def twice[n: Int]() -> Int` prints `8`
    at the pin. Mojito says "cannot be indexed here".
  - Closes R326 (`f[]()` read as a dereference), R72 (`Sized[Self.n + 1]` in
    an alias body, which now parses and fails in the check), and the parse
    half of R490. Shrinks R383 and R512, whose lowercase subscripts still
    need compile-time `__getitem__` folding.
  - Depends on R1370, so operands are already expressions.
  - Model: Fable, Planned.

- [ ] **R1372 A parameter binder is a value parameter only for a fixed list
  of type names**

  `ct_value_param_type` (`mojito-comptime/src/comptime/params.rs`) treats a
  binder annotation as a value type only for `Int`, `Bool`, `DType`, and a
  few others, and reads every other name as a trait bound, where upstream
  resolves the annotation like any type expression (`emitExprType`): a trait
  makes a type parameter, and anything else a value parameter of that type.
  - The lever: classify by the declaration table. A struct, or an alias of
    one, is a value type, and a trait is a bound.
  - Probe: `comptime MyInt = Int` then `def g[n: MyInt]()` makes `g[3]()`
    print `3` at the pin. Mojito says "unknown trait 'MyInt' in a
    type-parameter bound".
  - Closes R482 (a sized-scalar or alias-typed value binder is rejected),
    together with R1370.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R849 A `comptime if` over a scalar SIMD value parameter takes the
  wrong arm**

  Problem: `def g[x: Scalar[DType.uint8]](): comptime if x < 10: ...` takes
  the `else` arm for `g[3]()`, and so does `comptime if x == 3`; the pin
  takes the `then` arm.
  - The elaborator's `constraint_holds`
    (`crates/mojito-native/src/native/mono/availability.rs`) compares
    through `integer_value`, which reads only `Int`, `UInt`, and
    `IntLiteral` and answers `false` for anything else.
  - It should decide through `param_expr::fold`, the one folder, which now
    folds lane comparisons to a `bool` mask, or fail.
  - Found while landing R846 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R602 A local annotated `comptime` constant as a bracket value
  argument fails to elaborate**

  Problem: `comptime y: Int = 5` then `f[y]()`, for `def f[v: Int]() -> Int`, stops with "keeps the parameter constant `$comptime$main$c0()` in elaborated MIR"; the pin prints 5.
  - The call's instantiated arguments and its slot carry the application of the lifted constant (`param_apply { function: "$comptime$main$c0" }`), which reaches concrete MIR unanswered.
  - `comptime x = f[3]()` then `print(x)` runs.
  - `assets/ok/value_param_typed_by_type_param.mojo` fails on it.
  - Found while landing R601 (2026-10-10); HEAD before R601 fails the same way.
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

- [ ] **R509 An unread module constant constrained by an applied `where`
  operand is rejected**

  Problem: `comptime C: Int where (f(1) > 5, "big only") = f(1)` with
  nothing reading `C` runs at the pin and stops in Mojito with "unsupported
  generic constraint operand".
  - At the pin a non-generic constrained constant is a generator type,
    checked only where it is used.
  - `Elab::defer_constant` keeps refusing a constrained constant, so it is
    still evaluated where it is declared.
  - Probe: `conformance/probes/constrained_module_constant_unread.mojo`.
  - Found while landing R488 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R518 A generic `comptime` alias whose body applies a callable is
  refused**

  Problem: `comptime Twice[n: Int] = f(n)` bound by a compile-time constant
  (`comptime z = Twice[6]`, local or module) reports "a compile-time call
  of 'f' is not evaluated above the check", where the pin prints `12`.
  - A runtime read (`Twice[3]`, `Twice[k]`) and a parameter argument
    (`Arr[Twice[k]]()`) already run: the check binds the alias's template,
    which applies `f`, and `native::mono` evaluates it.
  - A compile-time constant is still folded above the check by
    `Elab::apply_generic_alias`, which folds no call. The lever is the one
    the module constants took: `Elab::applied_initializer` leaves an
    initializer applying such an alias to the check.
  - A struct's associated `comptime` member that applies a callable
    (`comptime k = f(1)` inside a struct) is refused with "not an associated
    comptime expression" and takes the same lever through
    `eval_associated_ct`.
  - Depends on nothing.
  - Model: Fable, Not Planned.

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

- [ ] **R471 A local `comptime` struct binding in a plain `def` is stored
  and destroyed**

  Problem: `comptime c = N(8)` then `print(c.a)` twice in `main`, over a
  `struct N` with a printing `__deinit__`, prints `8 / 8 / del 8` in
  Mojito, where the pin prints `8 / 8` and never destroys a `N`.
  - MIR gives `c` a frame slot that holds the frozen value and drops it, as
    if it were a `var`.
  - At the pin the binding is a parameter: a field read is a constant, and
    only a run-time use of the whole value materializes a temporary.
  - In a generic `def`, a value parameter's field read already folds.
  - Found while landing R467 (2026-10-07).
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
  - Found while landing R477 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R482 A sized-scalar value binder is rejected**

  Problem: `s[v: Int16]() -> Int16` is rejected with "unknown trait 'Int16'
  in a type-parameter bound", where the pin prints `6` for `s[5]()`.
  - The capitalized name in the binder's annotation is read as a trait
    bound, not as a value type.
  - Its argument `s[Int16(5)]()` also stops the parser with "Expected ']'
    after a subscript", a different shape from R381's operator.
  - Probe: a binder typed by a module type alias, `comptime MyInt = Int`
    then `def g[n: MyInt]()`, fails with "unknown trait 'MyInt' in a
    type-parameter bound", where the pin prints `3` for `g[3]()`. Prepare
    folds only aliases of `!kgen.type` and `!kgen.string` this way.
  - Probe: a struct's binder, `struct S[i8: Int8]` with `Self.i8` read in
    a static method, fails the same way (pin: `3` for `S[3].num()`).
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
  will be a run-time `if`, where the pin selects its arm per instance**

  `1 if x == 0 else 2 * f[x - 1]()` in `def f[x: UInt8]()` prints `8` on
  the pin for `f[3]()`: a sized integer's comparison is a parameter value.
  - `Checker::parameter_valued_condition`
    (`checker/comptime_validation.rs`) admits only `Int`, `UInt`, `Bool`,
    and literal operands, so once R482 accepts the binder, this
    conditional elaborates both arms and recurses past the instance budget.
  - A `Float64` operand stays run-time: the pin runs out of memory on the
    float twin of the probe.
  - Found while landing R887 (2026-10-14).
  - Depends on R482.
  - Model: Opus, Not Planned.

- [ ] **R1245 A local `comptime` conditional expression in a plain body
  is rejected**

  Problem: `comptime x = t[1] if c else t[0]` over `comptime t = (5, 1)`
  and `comptime c = t[0] == 1` fails with "not a compile-time value:
  unsupported compile-time expression", where the pin prints `5`.
  - The elaborator's evaluator (`crates/mojito-comptime/src/comptime/eval.rs`)
    has no arm for `ExprKind::IfExpr`, so a closed conditional cannot
    fold before the check.
  - Lever: evaluate the condition, then the selected arm only.
  - Found while planning R1139 (2026-10-20).
  - Depends on nothing.
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

- [ ] **R502 A parametric `comptime` alias in a function body is rejected**

  Problem: `comptime A[k: Int] = k + 1` inside a `def`, read as `A[10]`,
  fails with "a generic comptime alias must be declared at module scope",
  where the pin prints `11`.
  - The same alias at module scope runs.
  - Probe: `conformance/probes/local_generic_comptime_alias.mojo`.
  - Found while landing R310 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R475 A local type alias over `Scalar` or `SIMD` is rejected**

  Problem: `comptime S = Scalar[DType.float64]` then `S(2) * 1.25` in a
  plain `main` fails with "not a compile-time value: 'Scalar' is not a
  compile-time type", where the pin prints `2.5`.
  - In a lane-keyed `def`, `comptime S = Scalar[dt]` fails with "Undefined
    variable 'Scalar'" and `comptime S = SIMD[dt, 1]` with "Undefined
    variable 'S'".
  - The elaborator's `Elab::type_from_name` (`comptime/elab.rs`) has no
    type for a parametric alias or `SIMD`, and the checker's
    `comptime_type_operand` does not bind one either.
  - Found while landing R261 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R476 `comptime k = Self.n` in a generic struct's method is
  rejected**

  Problem: `comptime k = Self.n` (a bare member) in a method or static
  method of `struct S[n: Int]` fails with "type 'S[n]' has no associated
  type 'n'", where the pin prints `4` for `S[4]`.
  - `comptime_type_operand`'s `Self.` arm
    (`checker/comptime_validation.rs`) takes the member for a type.
  - R312 covers `Self.n + 1` cloning per instance; this is a rejection.
  - Found while landing R261 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1170 A struct's `comptime` type member read through `Self` in a
  method's compile-time position is rejected**

  Problem: over `struct Row[T: Movable]` with `comptime First = Self.T`, a
  static method's `comptime if Self.First == Int:` stops with "Undefined
  variable 'First'", and `comptime F = Self.First` with "'Self' is not a
  compile-time type", where the pin prints `10` for both at `Row[Int]`.
  - `Checker::constraint_operand`'s `Self.` arm takes every `Self.<name>`
    for one of the struct's own binders, never a member.
  - The pre-check elaborator's type resolution (`comptime/elab.rs`) has no
    type for `Self` in a method instance.
  - `var x: Self.First` in a method resolves.
  - Found while landing R805 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1180 A type comparison is not a compile-time `Bool` value**

  Problem: `comptime b = T == Int` and `materialize[T == Int]()` in `def
  g[T: Movable]()` stop with "Undefined variable 'T'", where the pin
  prints `True` for `g[Int]()`.
  - `comptime if T == Int:` runs, since `Checker::constraint_operand`
    reads a type operand. The value evaluators read `T` as a variable.
  - Probe: a local alias, `comptime F = Ts[0]` then `materialize[F ==
    Int]()` in `def f[*Ts: Movable]()`.
  - Found while landing R1168 (2026-10-20).
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

- [ ] **R443 An initializer list at a reflected field type is rejected**

  Problem: `var x: types[i] = {}` over `comptime types =
  reflect[T].field_types()`, under a `conforms_to` arm proving
  `Defaultable`, fails with "cannot emit initializer list for
  'reflect[T].field_types()[i]': the type has no construction to spell",
  where the pin prints each field's default.
  - `Checker::initializer_list_construction`
    (`checker/initializer_list.rs`) rewrites a brace into a spelled callee,
    and a reflected element has no name to spell.
  - The lever: have the brace record `SemanticAdjustment::ConstructType`,
    which MIR already lowers to `MirInstr::ConstructType`, instead of
    spelling a call.
  - `types[i]()` and `x: types[i] = types[i]()` run
    (`assets/ok/reflection_field_type_construction.mojo`).
  - Found while landing R364 (2026-10-07).
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
  - Past that, the AST unroller leaves `types[0]()` naming a dropped
    `comptime` binding ("Undefined variable 'types'"), the call-position
    loss R319 describes for an alias.
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

- [ ] **R368 A module `comptime` constant declared after the `def` that
  iterates it is undefined**

  Problem: `def f(): comptime for x in L: print(x)` followed by `comptime L
  = [10, 20]` fails with "Undefined variable 'L'", where the pin prints
  `10`, `20`.
  - The elaborator evaluates module constants in source order
    (`Elab::top_consts`), so the loop meets a name it has not bound yet.
  - With the constant declared first the loop runs, in a generic `def` too.
  - Found while landing R246 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R376 A compile-time application under an untaken `comptime if` arm
  is evaluated**

  Problem: `comptime if n > 100:` holding `comptime for z in [10 // (n -
  4)]:` in `def f[n: Int]()` stops `f[4]()` with "integer division or modulo
  by zero", where the pin skips the arm and runs.
  - `native::mono` unrolls every loop header of an instance before it
    selects a branch (`unroll_comptime_loops`, then
    `select_comptime_branches`, `mono/unroll.rs` and `mono/select.rs`), so
    the header's thunk runs in an arm the instance never takes.
  - Both walk regions since R880, so the fix makes `unroll_in` and
    `select_comptime_ifs` one in-order walk, as upstream's one work list:
    a `comptime if` is decided and its arm spliced before any `comptime
    for` in it is unrolled. Selection then moves before
    `substitute_function`, into `materialize_body`'s `answer_pending`
    loop.
  - Probe: `comptime if n > 0:` holding `comptime for i in range(10 //
    n):` in `def total[n: Int]() -> Int`; the pin prints `0` for
    `total[0]()`.
  - A nested `comptime if` whose condition applies a function fails the
    same way.
  - A local `comptime` display binding read in such an arm does too.
  - Found while landing reads of a local display binding (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R377 `materialize[L[1]]()` over a capitalized name is parsed as a
  type application**

  Problem: `comptime L = [1, 2]` then `print(materialize[L[1]]())` in `main`
  is rejected with "materialize[...]() takes one compile-time value", where
  the pin prints `2`.
  - The parser reads `L[1]` in a parameter argument as the type `L` applied
    to `1`, since the name is capitalized.
  - A lower-case name (`vals[1]`) is parsed as a subscript and runs.
  - Found while landing reads of a local display binding (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R381 A capitalized subscript followed by an operator does not parse
  in a parameter argument**

  Problem: `comptime L = [n, n * 2]` then `SIMD[DType.int32, L[0] * 2](5)`
  in `def f[n: Int]()` stops with "Expected ']' after a subscript", where
  the pin prints an eight-lane vector for `f[4]()`.
  - The parser reads `L[0]` in a parameter argument as the type `L` applied
    to `0`, since the name is capitalized, and a type takes no operator.
  - The bare `L[0]` is accepted: the check and MIR read the type
    application as the element (`display_element_argument`).
  - A lower-case name (`vals[0] * 2`) is parsed as a subscript and runs.
  - Found while landing a display binding read in a type or a parameter
    argument (2026-10-06).
  - Probe: `print(materialize[Ts[0] == Int]())` in `def f[*Ts: Movable]()`
    stops with the same "Expected ']' after a subscript" (found while
    landing R1168, 2026-10-20).
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
  - Found while landing a display binding read in a type or a parameter
    argument (2026-10-06).
  - Depends on nothing.
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

- [ ] **R386 A parameter argument that computes a `String` or a float from
  a parameter is rejected**

  Problem: `label[pick(n)]()` for `def label[s: String]()` and
  `scale[half(n)]()` for `def scale[x: Float64]()`, in `def f[n: Int]()`,
  are rejected with "not a compile-time Int constant: not an associated
  comptime expression", where the pin prints both.
  - The check lifts an `Int` or a `Bool` expression only
    (`Checker::lifted_application`, `checker/comptime_validation.rs`).
  - A lifted function returning a `String` or a `Float64` already serves a
    `comptime for` display, so the elaborator can freeze either value.
  - The parameter domain needs a value meta for each, and `UInt` and the
    sized integers want the same.
  - Found while landing an application in a type or a parameter argument
    (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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
  - Found while landing R789 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R489 An associated `comptime` member that applies a function is
  rejected**

  Problem: `comptime V = f(Self.k)` in `struct S[k: Int]`, read as
  `Self.V`, prints at the pin, while Mojito reports "not a compile-time Int
  constant: not an associated comptime expression".
  - A function body's local binding of the same call is a request the
    elaborator serves per instance (`docs/notes/ctfe-request-path.md`).
  - The member is the generator's, evaluated under the instance's bindings,
    so it can denote the same application.
  - Probe: `comptime X = K(1)` in `struct K`, matched as `case K.X:`
    (an equality test over a subject that is not `EnumLike`), fails the
    same way; the pin prints `x`. `case .X:` also needs R1238
    (`conformance/probes/leading_dot_equality_operand.mojo`).
  - Found while planning R7 (2026-10-07).
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

- [ ] **R1206 The pre-check elaborator does not read a struct member's
  sibling members**

  Problem: over `struct S[T: AnyType]` with `comptime a = 2` and `comptime
  b = Self.a + 1`, `comptime m = S[Int].b` stops with "'Self' is not a
  compile-time type", where the pin prints `3`.
  - `Elab::associated_value_at` (`comptime/elab.rs`) evaluates a member's
    body over the struct's parameters and the member's own, but not the
    struct's other members, so `Self.a` falls to an unknown `Self`.
  - The check reads the same member at run time: `print(S[Int].b)` prints
    `3`.
  - Probe: through the unbound struct, `comptime m = S.b` fails the same
    way, where the pin prints `3`.
  - Probe: a member depending on the parameters through a sibling,
    `comptime a = Self.T` and `comptime b = Self.a`, makes `comptime m =
    S.b` fail the same way, where the pin reports "cannot access comptime
    member 'b' with unbound parameter 'S.T'". `Elab::generator_member`
    tests only the body's own spelling for the parameters.
  - Probe: a member passing a sibling as a bracket value, `comptime m =
    Box[Self.n].w` beside `comptime n = 3`, read as `comptime x = S.m`,
    fails with "value parameter 'n' expects a compile-time Int, got
    Self.n", where the pin prints `30`; `print(S.m)` prints `30`.
  - Probe: a parameterized value member reading a sibling, `comptime get[i:
    Int]: Int = i + Self.K` beside `comptime K = 100` (or taken from a
    trait's default), read as `comptime k = A.get[2] + 1`, fails the same
    way, where the pin prints `103`; `print(A.get[2])` prints `102`.
  - Found while landing R1204 (2026-10-20).
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
  - The expression evaluators have no arm for it: `eval_associated_ct`,
    `compile_dependent_ct_expr`, and `constraint_operand`
    (`checker/constraints.rs`), and a local `comptime` binding's.
  - The rejections differ by site: "function types as compile-time
    values", "Undefined variable 'W'", "unsupported dependent parameter
    expression", and "'Self' is not a compile-time type".
  - Probe: a type pack's member spelled out, `comptime E =
    Ts.__getitem_param__[1]` ("Undefined variable 'Ts'"), and `comptime G
    = TypeList[Ts.values].__getitem_param__[1]`, `comptime if
    Ts.__getitem_param__[0] == Int:`, and a struct member `comptime First =
    Self.Ts.__getitem_param__[0]`.
  - Probe: a member applying another struct's alias, `comptime _mlir_type
    = _MLIR.KGENParamListType[Self.type]`, stops packaging with "not an
    associated comptime expression". It keeps `_mlir_type` off the bundled
    `ParameterList` and `TypeList`.
  - Probe: the same over a user struct, `comptime Same = R.Of[Bool]` beside
    `comptime Of[T: AnyType] = Tuple[Int, Int]`, fails with "not an
    associated comptime expression", where the pin runs `var r: R.Same`.
    A parameterized member's body (`comptime Same[T: AnyType] = R.Of[T]`)
    reads it since R806.
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
    and elaborated MIR rejects a register, slot, or signature that holds
    one (`verify/concrete.rs`).
  - Probe: a runtime parameter `def f(x: __mlir_type.`!kgen.string`)`
    called as `f("a".value)`, which the pin runs.
  - Probe: `var l = List[KGENString]()` then `l.append("a".value)` and
    `print(len(l))`, which prints `1` at the pin. Mojito stops only in
    elaborated MIR ("holds the compile-time-only metatype").
  - Probe: `ParameterList[names]()[0]` over `*names: KGENString`, which
    reads the elements' runtime storage through `get_span`.
  - A sibling of R1149's raw-list runtime form.
  - Found while landing R1183 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1191 A `comptime` string constant is not read in a bracket
  argument**

  Problem: `comptime lit = "So"` then `bind[lit]()` over `def bind[s:
  StringLiteral]()` fails with "not a compile-time Int constant: lit",
  where the pin prints `So`.
  - The `ExprKind::Identifier` arm of `Checker::eval_associated_ct`
    (`checker/constraints.rs`) reads only `Int` constants (`comptimes`),
    applied constants, and `module_values`, which holds only
    `is_folded_parameter_argument` values.
  - A module string constant sits in `comptime_literals`, and a local one
    is an ordinary immutable binding, so neither is a compile-time value
    there.
  - Probe: a local `comptime local = "Lo"` then `bind[local]()`.
  - Probe: `bind[lit + "me"]()`, which prints `Some` at the pin.
  - Probe: the literal's KGEN string read through one,
    `kbind[(lit + "me").value]()` and `kbind[local.value]()` over `def
    kbind[s: KGENString]()`.
  - Found while landing R1183 (2026-10-20).
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

- [ ] **R1194 A member read through a `TypeList` construction in a local
  `comptime` is lifted as a function**

  Problem: `comptime n = TypeList[Ts.values]().length` in `def h[*Ts:
  AnyType]` stops with "MIR function '$comptime$h$c0' block 0 parameter
  constant names parameter `Ts` of `h` that no enclosing declaration
  binds", where the pin prints `3` for `h[Int, Bool, Int]()`.
  - `TypeList[Ts.values].length`, without `()`, runs, and so does `return
    TypeList[Ts.values]().length`.
  - `compile_dependent_ct_expr` (`checker/constraints.rs`) reads a member
    through a type (`type_member_value` over `member_subject`), but
    `member_type_operand` gives up on a construction.
  - The binding is then lifted to a thunk, which does not bind the `def`'s
    pack.
  - Upstream reads a `comptime` member through an instance on the
    instance's type.
  - Probe: a value alias's body over a construction, `comptime Count[*Ts:
    AnyType] = TypeList[Trait=AnyType, Ts.values]().length`, is rejected
    at the declaration with "a variadic pack on a Bool-bodied comptime
    alias parameter", where the pin prints `3` for `Count[Int, Bool,
    Float64]`: `Checker::lower_parametric_value` reads the body through
    `compile_dependent_ct_expr`, which gives up on the construction.
  - Probe: a struct member's body over a construction, `comptime n =
    TypeList[Trait=AnyType, Self._types]().length`, fails with
    "unsupported associated comptime member access", where the pin prints
    `2` for `S[Int, Bool].n`; `eval_associated_ct` reaches the same
    `member_type_operand`.
  - Found while landing R1184 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Not Planned.

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

- [ ] **R1218 The pre-check conformance oracle registers traits without
  their `comptime` requirements**

  Problem: `struct W[T: Tr]` with `var b: Box[Self.T.K]` and an initializer
  taking one, used as `W[S](Box[3]())`, stops with "could not build the
  specialization conformance oracle: 'Tr' value has no attribute 'K'",
  where the pin prints `ok`.
  - `ConformanceOracle::importing` (`checker/conformance.rs`) registers
    every trait with empty `comptime_members`, then resolves struct field
    types, so a field reading a bound's value member finds no requirement.
  - The same type as an instance method's parameter runs; R1179 stops at
    the same site for a different cause (a header `where` over a pack).
  - Once the oracle builds, a field read at `W[S]` types through
    `substitute_at` alone (`checker/indexing.rs`), which leaves `S.K`
    unanswered; it needs `resolve_assoc_ty`, as the method parameter paths
    in `method_calls/` now take.
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

- [ ] **R1203 A `comptime` name bound to a `TypeList` value is not
  subscripted in type position**

  Problem: `comptime tl = TypeList.of[Trait=AnyType, Bool, Int]()` then
  `var x: tl[1] = 3` fails with "unknown type 'tl'", where the pin prints
  `3`.
  - A type subscript reads a value's `__getitem_param__` only when the
    value is a construction written in place
    (`Checker::compile_time_value_type`, `checker/type_resolution.rs`).
  - A named value parses as `Type::Named`, so the checker never asks
    whether the name is a local `comptime` value. `typelist_receiver`
    follows `local_comptime_value` for the same question in a constraint.
  - The interplay with R1194's lifted local `comptime` needs settling here.
  - Found while landing R1185 (2026-10-20).
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
  - The elaborator's alias table (`TypeAliases` in `comptime/elab.rs`)
    holds only types, and `Elab::eval` reads a trait name as an unknown
    type.
  - The checker's side needs a module or local constant whose value is
    `CtValue::Trait`, read by `Checker::trait_value` wherever a trait is
    spelled.
  - Using such an alias as a bound (`T: MyTr`) is R1167's bound form, not
    this entry's.
  - Probe: `comptime MyTr = Copyable & Movable`, then `print(g[MyTr]())`,
    prints `1` at the pin.
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

- [ ] **R808 A `__generator_type[...] T` parameter and an unbound
  application `Alias[a, _]` are not supported**

  Problem: upstream passes parameterized `comptime` aliases as generator
  values to `ParameterList.tabulate`, `reduce`, `any`, `all`, and
  `map_to_type`, and Mojito parses neither spelling.
  - `__generator_type` appears nowhere in `crates/`.
  - Mojito's `TypeList.any`/`all` predicates are checker builtins
    (`checker/constraints.rs`), not a parameter type.
  - The work covers the parser (`docs/grammar.md` first), the checker's
    parameter type, and the elaborator's application of a generator.
  - Probe: a parameterized member bound unapplied as a generator value,
    `comptime g = T.get` then `g[4]` through a bound `T: HasGet`, prints
    `8` at the pin, where Mojito rejects the read with "type 'T.get'
    expects 1 type argument(s), got 0"; the concrete `comptime g = A.get`
    is rejected likewise.
  - A session should plan this first and split it if it does not fit.
  - Found while planning R493 (2026-10-13).
  - Depends on R801, since a generator's body is a parameterized `comptime`
    value.
  - Model: Fable, Planned.

- [ ] **R802 A struct's `comptime` member over the length of its own value
  pack is rejected**

  Problem: `comptime size: Int = len(Self.values)` in `struct PL[*values:
  Int]`, read as `PL[1, 2, 3].size` or as `Self.size` in a method, prints
  `3` at the pin, while Mojito reports "not an associated comptime
  expression".
  - `Self.values` is a `ParameterList` value, so the member applies its
    `__len__` at compile time, which a struct member cannot do yet (R489).
  - Only the bundled `ParameterList` reads its own raw list's length
    (`Checker::bundled_raw_list_operand`).
  - Found while landing R494 (2026-10-13).
  - Depends on R489, which lets a struct member apply a function.
  - Model: Opus, Not Planned.

- [ ] **R396 A signature that calls a function over a value that is no
  `Int` or `Bool` is rejected**

  Problem: `def gen[n: Int]() -> SIMD[DType.int32, Int(half(n))]:` for `def
  half(x: Int) -> Float64`, and `-> SIMD[DType.int32, P(n).get()]` for a
  method `get` of `struct P`, are rejected with "not a compile-time Int
  constant: not an associated comptime expression", where the pin runs both.
  - A call is the application of its callee by name only when every
    parameter and the result is an `Int` or a `Bool`
    (`applicable_functions`, `checker/comptime_validation.rs`).
  - A method call's receiver is a struct value, which is no such argument.
  - In a body each of these is lifted whole, so it runs there, as a
    different value from the same call spelled in another declaration.
  - A struct value in the parameter domain can take the frozen form a
    struct-typed parameter argument already has.
  - Found while landing the callee shapes a signature applies (2026-10-06).
  - Depends on R386, which gives the parameter domain a value meta for a
    `Float64` and a `String`.
  - Model: Fable, Not Planned.

- [ ] **R397 A signature that calls a function generic over a type is
  rejected**

  Problem: `def gen[n: Int]() -> SIMD[DType.int32, idt(n)]:` for `def
  idt[T: Copyable](x: T) -> T` is rejected with "not a compile-time Int
  constant: not an associated comptime expression", where the pin runs it.
  - A callee's own compile-time parameters are applied by name when each is
    an `Int` or a `Bool` the call spells (`twice[n]()`).
  - A type parameter, a parameter the call leaves to inference, and one
    with a default are left out (`ApplicableShape::applicable`,
    `checker/comptime_validation.rs`).
  - A default argument that reads a name the calling declaration binds too
    is left out as well, since the default belongs to the callee's scope
    (`Checker::applied_default`).
  - In a body such a call is lifted whole, so it runs there.
  - Found while landing the callee shapes a signature applies (2026-10-06).
  - Depends on nothing.
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
  - A static method of a generic struct prints the instance it is called on
    as an argument: `G.w[G[2]]()` for `G[2].w()`.
  - `not (n > 9)` prints as `not 9 < n`.
  - The application holds the MIR declaration's name and every argument in
    one list (`ParamKind::Apply`, `param_expr.rs`), so the printer knows
    neither the source name nor which arguments are compile-time ones.
  - Found while landing the callee shapes a signature applies (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R512 A subscript of a module list or tuple constant in a parameter
  argument is rejected**

  Problem: `show[XS[1]]()`, `show[T[1]]()`, and `SIMD[DType.int64,
  xs[1]](7)` over module constants `comptime XS = [1, 4, 5]` and
  `comptime T = (2, 3)` are rejected, where the pin runs all three.
  - A capitalized constant (`XS[1]`, `T[1]`) parses as a type application
    and is rejected with "type mismatch for value parameter 'n': expected a
    value, found a type", or "SIMD width … got a type".
  - A lowercase one (`xs[1]`) is rejected with "not a compile-time Int
    constant: xs".
  - It does not matter whether the constant is folded where it is declared
    or waits for its first demand.
  - R383 reports the local display binding's spelling of the same parse.
  - Found while deferring display and subscript module constants
    (2026-10-08).
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

  Problem: six sites bind a bracket list to a declaration's parameters by
  their own rule instead of `mojito_ast::call::match_parameter_slots`, the
  pin's rule the checker's `bracket_bindings` uses.
  - `callable_value_argument` (`checker/origins/subst.rs`) binds positional
    arguments over infer-only binders, and skips a keyword-filled slot.
  - `applied_arguments` (`checker/comptime_validation.rs`) pairs a binder
    with the argument at its raw position.
  - `mir::supplied_parameter_arguments` (`mir/ir.rs`) is a separate copy.
  - `verify_param_arguments` (`mir/verify/subscripts.rs`) lets a
    positional argument skip a keyword-filled slot, which the pin rejects.
  - `freeze_arguments` (`comptime/elab.rs`) counts keywords when indexing
    the explicit list.
  - Probe: `T[k=3, Extent.square(2)]` over `struct T[e: Extent, k: Int]`
    prints the pin's `23` only because CTFE later evaluates the unfrozen
    construction.
  - `Elab::type_from_name` (`comptime/elab.rs`) binds a list positionally
    without skipping infer-only binders. Probe: `comptime n =
    TypeList[S[Int, Bool]._types].length` in `main` stops with "type
    'TypeList' expects 2 compile-time argument(s), got 1", where the pin
    prints `2`; so does `comptime n = ParameterList[c].size` over a
    module `comptime c = ParameterList.of[1, 2].values` ("type
    'ParameterList' expects 2 compile-time argument(s), got 1"). R207 is
    the variadic half of the same function.
  - `Elab::bind_subscript_arguments` (`comptime/eval.rs`) binds a user
    struct's member application positionally, with no infer-only binder
    or pack. Probe: over `struct PL` declaring upstream's `comptime
    of[type: AnyType, //, *values: type] = ParameterList[type=type,
    values.values]`, `comptime Ints = PL.of[4, 5, 6]` stops with "a
    compile-time parameter bound twice or past the last", and
    `PL.of[4, 5, 6]()` with "Undefined variable 'PL'", where the pin
    prints `(4, 5, 6)`; the bundled `ParameterList.of` binds through
    `Checker::bind_closed_bracket_list`.
  - MIR lists carry erased origin arguments with no value, which the MIR
    adapters must keep aligned.
  - Found while landing R449 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R450 A `UInt` value parameter inferred from an argument type is missing at the call**

  Problem: `name(S[UInt(3)]())` over `def name[v: UInt](s: S[v]) -> UInt`
  fails with "required compile-time value parameter 'v' is missing", where
  the pin prints `3`; the same holds for `v: T` solved to `UInt`.
  - MIR's `inferred_param_arg_regs` passes an inferred `Int`, `Float64`,
    `Bool`, string, or `DType` value as a constant register, but has no
    constant for a `UInt`.
  - Mono already reads a bound `UInt` as its 64 bits in a `UInt` slot, which
    the register needs to match.
  - Found while landing R446 (2026-10-07).
  - Depends on nothing.
  - Model: Fable, Not Planned.

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
  arguments" and "an empty subscript ('value[]') is the pointer
  dereference", where the pin binds the empty list and prints `0`.
  - The parser reads `total[]` as the pointer dereference `p[]`, so the
    call must tell a generic callee's empty brackets from it.
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

- [ ] **R464 A compile-time value of a struct without fieldwise
  construction is rejected**

  Problem: `comptime r = R(3)` over a `struct R` (`a: Int`, `s: String`)
  whose only `__init__(out self, a: Int)` sets `self.s = String(a)` fails
  with "a compile-time 'R' value needs fieldwise construction", where the
  pin prints `r.s` as `3`.
  - The VM freezes the result (`VmBackend::freeze`), but
    `Elab::check_frozen_structs` (`comptime/ctfe.rs`) refuses it, since a
    frozen `CtValue::Struct` comes back as a call to the fieldwise
    constructor.
  - Mojo materializes the parameter attribute directly, with no
    constructor call.
  - Found while landing R456 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R465 A compile-time construction of a generic struct is rejected**

  Problem: `comptime g = G[Int](5, "gen")` over `@fieldwise_init struct
  G[T: Copyable & Movable & Writable & Deinitable]` fails with "'G' is not
  a compile-time-callable function", where the pin prints `5 gen`.
  - The same happens without the `String` field.
  - The elaborator routes a parameterized callee to CTFE of a function by
    that name rather than to `Elab::ctfe_struct_entry`.
  - Found while landing R456 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R461 A `String(...)` construction in a parameter argument is
  rejected**

  Problem: `g[String("a")]()` for `def g[s: String]()` and
  `g[(1, String("a"))]()` for `def g[p: Tuple[Int, String]]()` fail with
  "not a compile-time Int constant: not an associated comptime expression",
  where the pin prints `a` and `1 a`.
  - `Checker::eval_associated_ct` does not evaluate a `String` construction,
    and the lifted fallback serves `Int` and `Bool` only.
  - The literal spellings (`g["a"]`, `g[(1, "a")]`) run.
  - R386 is the same gap over an open parameter (`label[pick(n)]`).
  - Found while landing R455 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R462 A tuple argument over an enclosing binder is rejected**

  Problem: `g[(n, 2)]()` inside `def f[n: Int]()`, against `def g[p:
  Tuple[Int, Int]]()`, fails with "type mismatch for value parameter 'p':
  expected Tuple[Int, Int], found (n, 2)", where the pin prints `5 2` for
  `f[5]()`.
  - The argument stays a symbolic `CtValue::Expr`, which `ct_value_ty` has
    no tuple type for, and contextual typing is tried only for a closed
    value (`Checker::converts_to_parameter`).
  - A scalar argument over the binder (`h[n + 1]`) runs.
  - Found while landing R455 (2026-10-07).
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
  - Found while landing R1401 (2026-10-26).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R326 An empty explicit application `f[]()` is a pointer dereference**

  Problem: `none[]()` over `def none[*vals: Int]() -> Int` fails with "an
  empty subscript ('value[]') is the pointer dereference", where the pin
  binds the pack empty and prints `0`.
  - The parser reads `name[]` as `EmptySubscript` whatever `name` denotes;
    the checker would type it as an application when the callee is a
    generic `def`.
  - Probe: `g[]()` over `def g[Tr: type_of(AnyType), //, *Ts: Tr]()`,
    which the pin rejects with "failed to infer parameter 'Tr'" (R1176).
  - Found while landing R318 (2026-10-05).
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

- [ ] **R337 A computed value argument in a bracket is rejected**

  Problem: `W[fib(6)]()` for `struct W[n: Int]`, or `g[fib(6)]()` for `def
  g[n: Int]()`, prints `8` at the pin, while Mojito reports "not a
  compile-time Int constant: not an associated comptime expression".
  - `eval_associated_ct` (`checker/constraints.rs`) has no arm for a call;
    a module constant over the same call (`comptime N = fib(6)`) folds
    before the check and works.
  - The pin keeps such an application symbolic through the check
    (`ParamKind::Apply`), so it is no frozen literal; a struct-typed
    argument is frozen to its fieldwise construction
    (`Elab::freeze_struct_value_arguments`), which a scalar one should not
    copy.
  - Found while landing R4's struct-valued half (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R338 A static call through a `comptime` alias of a generic struct is
  rejected**

  Problem: `comptime W3 = W[3]` then `W3.total()`, a `@staticmethod` of
  `struct W[n: Int]`, prints `3` at the pin, while Mojito reports "function
  types as compile-time values".
  - The receiver reaches `infer` as a `TypeValue`, which has no
    expression type; a construction through the alias (`W3()`) works.
  - Found while landing R4's vector-keyed half (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R603 A static call through a `comptime` alias of any type is
  rejected, not just a generic struct's**

  Problem: `comptime A = S` then `A.k()`, a `@staticmethod` of a
  non-generic `struct S`, prints `7` at the pin, while Mojito reports
  "function types as compile-time values".
  - A closed `Tuple` alias fails the same way: `comptime T = Tuple[Int,
    String]` then `T.__len__()` (pin: `2`), at local or module scope.
  - In a pack def, `comptime T = Tuple[*Ts]` then `T.__len__()` stops
    earlier, with "Undefined variable 'T'" (pin: the length); the same call
    spelled `Tuple[*Ts].__len__()` runs.
  - The aliased receiver reaches `Checker::infer_impl` as
    `ExprKind::TypeValue`, as R338 describes for a generic struct; the fix
    is to resolve an alias receiver as a static receiver for every type.
  - Found while landing R347 (2026-10-10).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R1211 A compile-time condition that is not a `Bool` is not
  converted**

  Problem: `comptime if 3:` and `comptime assert 3, "int"` run at the pin,
  which converts the `Int` through `Bool(x)`; Mojito fails with "invalid
  checked program: MIR function '$comptime$main$0' block 0: return of Int
  from a function returning Bool".
  - The checker accepts the condition and records its truthiness
    conversion (`truthiness_condition`), but the compile-time thunk the
    condition lowers to (`comptime_condition_of`) returns the operand
    unconverted.
  - Probe: `comptime if 3:` in `main`.
  - Probe: `comptime assert 3, "int"` in `main`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R569 A reflected field type over a pointer field is rejected**

  Problem: `reflect[Box].field_types()` or `reflect[Box].field_at[0]` on a
  struct holding `var data: UnsafePointer[Int, MutUntrackedOrigin]` runs at
  the pin, while Mojito reports "not a compile-time value: 'UnsafePointer'
  is not a compile-time type".
  - The elaborator types an own struct's field annotations itself
    (`Elab::reflected_field_type`, through `type_from_anno`), which cannot
    resolve `UnsafePointer`.
  - A package struct's field types are read as its check resolved them, so
    `reflect[List[Int]].field_types()` works when the bundled library is
    imported and fails from source (`MOJITO_STD_PACKAGE=0`).
  - The lever: read every reflected field type from the conformance
    oracle's checked struct table, as the package path does.
  - Found while landing R559 (2026-10-10); no fixture pins it yet.
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R513 A `comptime for` over another struct's compile-time list is
  rejected**

  Problem: `comptime for v in K.L`, where `struct K` declares `comptime L =
  [1, 2, 3]`, sums the list at the pin, while Mojito reports "Undefined
  variable 'K'".
  - It fails the same way in a plain function and in a generic struct's
    method, so it is the loop header's reading of a type's member, not a
    template gap.
  - `Self.L` over the struct's own list is R279's spelling.
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
    through it. Mojito's `Hashable` default is synthesized
    (`comptime/synth.rs`), so no bundled body needs it yet.
  - Probe: `print(reflect[P].field_ref[0](p), reflect[P].field_ref[1](p))`
    over `P(3, 4)` (pin: `3 4`).
  - Probe: a generic `total[T](x: T)` summing `rebind[Int](r.field_ref[i](x))`
    under `comptime if types[i] == Int` in a `comptime for` reports
    "Undefined variable 'r'" (pin: the sum).
  - Probe: `reflect[P].field_ref[0](p) = 42` also needs R1233's call
    assignment target.
  - Depends on nothing.
  - Model: Fable, Planned.

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

- [ ] **R485 An `Int`-annotated local `comptime` accepts a `Bool` value**

  Problem: `comptime m: Int = 1 > 0` in a function prints `True`, where the
  pin rejects it with "cannot implicitly convert 'Bool' value to 'Int'".
  - The checker's `StmtKind::Comptime` arm (`checker/statements.rs`) drops
    an `Int` annotation before typing the value, so the binding keeps the
    value's own type.
  - A template body's binding over its binders checks its annotation
    (`bind_template_comptime`) and is rejected as at the pin.
  - Same annotation-dropping family as R98.
  - Probe: `conformance/probes/comptime_int_annotation_bool_value.mojo`.
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

- [ ] **R498 A nested `def` keyed on a type pack fails MIR verification**

  Problem: `def inner[*Ts: Writable](*args: *Ts)` declared inside `def
  outer()` and called there (`inner(1, "a")`) stops with "callable contract
  names parameter `*Ts` of `inner` that no enclosing declaration binds",
  where the pin runs it.
  - The same `def` at module level is served by its template and runs.
  - Found while landing R253 (2026-10-08).
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

- [ ] **R543 A local `comptime` application that returns a
  reference-holding value fails CTFE even when nothing reads it**

  Problem: `comptime ks = d.keys()` over a local `comptime d = {"a": 1}` in
  `main` fails with "VM CTFE returned unsupported runtime value <ref 0:1>",
  where the pin accepts the program.
  - The result is a `_DictKeyIter`, which holds a reference into the
    dictionary, and a CTFE result cannot carry a reference.
  - The local binding is demanded although no read names it. The module
    form (`comptime KS = D.keys()`) is demanded lazily and passes.
  - `comptime for k in d.keys()` is R321's iterator gap, not this one.
  - Found while R539 kept local displays for the check (2026-10-09).
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

- [ ] **R1179 A struct header's `where` clause over the struct's own type
  pack stops the pre-check elaborator**

  Problem: `struct P[*Ts: Movable] where Ts.length == 2` used as
  `P[Int, Bool].two()` stops with "could not build the specialization
  conformance oracle: unsupported dependent parameter expression", where
  the pin prints `2`.
  - `where Ts.all_conforms_to[Copyable]()` in the same place stops with
    "unsupported generic where proposition", where the pin prints `2`.
  - Both stop while `ConformanceOracle::importing`
    (`checker/conformance.rs`) builds the oracle, before any check runs.
  - A method's own `where Self.Ts.length == 2` runs.
  - Found while landing R1168 (2026-10-20).
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

- [ ] **R486 A compile-time evaluation cannot write a `Tuple` or a `t"…"`
  literal**

  Problem: `comptime s = label(3)`, where `label` returns `String(t"n={n}")`
  or `String((n, 2))`, prints at the pin and stops in Mojito with
  "TString.write_to: unspecialized type-keyed method" (or `Tuple.write_to`).
  - The evaluation's subprogram carries a variadic struct's `comptime for`
    member as its template stub and runs it erased, as R131's does.
  - A t-string there ran until 2026-10-07 only through MIR's eager
    concatenation fallback, deleted when the checker began building the
    `__make_tstring` call.
  - Entry R7 serves the instance from the worklist and closes this.
  - Probe: `conformance/probes/ctfe_writes_tuple_and_tstring.mojo`.
  - Depends on R7.
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

- [ ] **R134 `type_of(x)` of a value is not accepted as a type**

  Problem: `var y: type_of(x) = 3` stops with "'type_of' of a value or a
  struct type is not supported yet", and `unsafe_alloc[type_of(px)](1)` with
  "expected a type, found a value", where the pin runs both.
  - The parser reads `type_of(...)` as `Type::TypeOf`, and the checker
    (`ty_from_anno`) accepts only a trait operand, whose type is the trait
    metatype.
  - `type_of(Int)`, the metatype of a struct type, is the same gap.
  - A bracket argument spelled `type_of(px)` parses as a call expression,
    not a type.
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
  - A legacy `@parameter` closure follows R533's environment path, not
    R1376's storage struct.
  - Depends on R533.
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

- [ ] **R340 A `comptime for` header cannot call a static method on `Self`**

  Problem: `comptime for i in range(Self.__len__())` in a variadic struct's
  method reports `Undefined variable 'Self'`, where the pin accepts it and
  upstream's `Tuple` spells every element loop that way.
  - Mojito's `tuple.mojo` keeps `range(len(Self.Ts))`.
  - The header is evaluated by the elaborator, which binds no `Self` for a
    static call.
  - Found while landing the `Tuple` default initializer (2026-10-05).
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
  - R279 is the same member read as `Self.k` inside the struct.
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

- [ ] **R853 A function-body `comptime` binding of a sized-scalar
  construction is refused**

  Problem: `comptime d = UInt8(250)` in a function body fails with "a
  compile-time call of 'UInt8' is not evaluated above the check", while the
  pin prints `250`.
  - `comptime d = UInt8(250) + 10` and `comptime c = UInt8(7) // UInt8(0)`
    fail the same way.
  - A `comptime a: UInt8 = 250` binding already works, and its operators
    now fold.
  - R518 is a different shape (a generic alias body).
  - Found while landing R846 (2026-10-14).
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

- [ ] **R1403 Nineteen entries in `divergences`, `pmir`, `native`,
  `code-org`, and `tooling` name gone code or predate the work that touched
  them**

  These entries' causes were written before later work reworked their code,
  so a session reading them looks for functions that no longer exist.
  - Partly fixed: R862. Its `conforms_to` probe prints the pin's `False True
    True False`, and `struct_trait_rule` maps `TrivialRegisterPassable` to
    `Declared` since R1267. Only the bound half remains, which R1363 closes.
  - Cite gone code: R245 (`unroll_comptime_for`, and R246 landed), R163
    (`generate_def_spec`), R1018 (`Prog::selected_struct_member`), and R423
    (`$write_formatted`).
  - Wait on landed entries: R162 (R7) and R182 (R27).
  - Overtaken: R1144 by R1329's copy elision, and R173
    (`try_region_drop_timing`) by gap A's explicit raise exits.
  - Duplicate: R1145 (`ref z = Int()`) restates R169's `RefDecl`
    materialization fallback.
  - `pmir`: R1279, filed before R1283 and R1278 reworked copy and
    constructor synthesis. Add `Depends on R533` to R983.
  - `native`: R633 (`VariadicPack(Int)`), filed before R743, and R299 (an
    element struct layout), filed before R1102 and R1073.
  - `code-org`: R457 (`checked_var_types` is gone, and so is R147's bullet
    naming it) and R521 (`check_program_for_discovery` is gone,
    `DiscoveryResult` remains).
  - `tooling`: R823's version numbers are stale. The snapshots read 1.98,
    and the reader accepts up to 1.102.
  - Add `Depends on R533` to R172: its elaboration half follows R533's
    environment path.
  - For each entry, run its probe (or the program its first sentence names)
    on `target/debug/mojito` and on the pinned Mojo, through the Pixi
    project `docs/usage.md` names (`MOJO_PIXI_MANIFEST`).
  - Delete an entry whose probe now matches the pin, and record it in
    `CHANGELOG.md`.
  - Rewrite an entry that still fails so its cause names today's code, and
    keep its ID.
  - Merge a duplicate into the entry it restates, carrying its probes, and
    delete it.
  - Done when every entry listed here is deleted, rewritten against today's
    code, or merged.
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

- [ ] **R1144 A copy that is its source's last use stays a copy**

  Problem: `var u = t` with no later use of `t`, and `match make(): case
  var x:` over a temporary, print `copy` and two `del`s in Mojito, where
  the pin moves the dying source into the binding and prints one `del`.
  - `analysis/copy_elision.rs` keys on a `DropVar` directly after the copy
    construction; the binding's `var.store` comes first here, and a match
    binding reads through the hidden `$match` slot.
  - The `var u = t` form now moves as at the pin (R136's fixture
    `assets/ok/implicit_copy_last_use_forwards.mojo`); the match-binding
    forms remain.
  - Probes: `conformance/probes/match_binding_kinds_on_temporary.mojo`,
    `match_guard_copy_drop.mojo`, and
    `match_temporary_subject_survives_failed_var_case.mojo` keep the traces;
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

- [ ] **R1145 `ref z = Int()` binds a register-passable temporary**

  Problem: `ref z = Int()` runs in Mojito, where the pin rejects it: `value
  of type 'Int' doesn't have a memory origin in 'ref' binding`.
  - A `match` applies the rule to `case ref z` over a temporary subject
    (`bind_subject` in `checker/patterns.rs`, `TypeError::NoMemoryOrigin`);
    the plain `ref` declaration (`statements.rs`, the `RefDecl` arm) still
    materializes the temporary into a hidden slot.
  - Fixture: `assets/type_error/match_ref_binding_register_temporary.mojo`
    is the match form; add the plain form beside it.
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

- [ ] **R1018 A struct value is accepted where a runtime function type is
  expected**

  Problem: `apply(Counter(10), 1)` for `def apply(f: def(Int) -> Int, x:
  Int)` runs on Mojito, where the pin rejects it ("value passed to 'f'
  cannot be converted from 'Counter' to 'def(Int) -> Int'"), for a
  parametric and a non-parametric struct alike.
  - The VM runs it through a `__trait_dispatch.__call__` contract
    (`Prog::selected_struct_member`), and Pliron segfaults on it.
  - R1009's `discover_structs` `__call__` demand exists only for this path.
  - Fix: reject the conversion in the checker, with an
    `assets/type_error` fixture and its
    `conformance/assets-mojo-errors.tsv` row.
  - Found while planning R1003 (2026-10-15).
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
  - `docs/features.md`'s scalar-operation-traits row names them as
    supported bounds.
  - Found while landing R709 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R577 A struct conforms to a compiler-known trait it never
  declares, such as `Boolable`**

  Problem: `comptime if conforms_to(T, Boolable)` holds for a plain
  `@fieldwise_init` struct in Mojito, where the pin answers no.
  - Cause: the shared rule answers every compiler-known trait outside its
    declared, lifecycle, and operation classes as always held
    (`StructTraitRule::Shallow` in `mojito_types::conformance`, and the
    checker's `conforms_to` `_ => true` arm).
  - Upstream decides these from the type's declared conformances, as any
    trait.
  - The lever: turn `Shallow` into `Declared` once the bundled library
    declares what upstream's does (R578).
  - Pinned by `conformance/probes/conforms_to_undeclared_compiler_trait.mojo`.
  - Found while landing R564 (2026-10-10).
  - Depends on R578.
  - Model: Opus, Not Planned.
- [ ] **R862 `TrivialRegisterPassable` holds for a struct that never
  declares it**

  `conforms_to(T, TrivialRegisterPassable)` holds for a `Movable` struct and
  `List[Int]`, and a `T: TrivialRegisterPassable` bound accepts both, where
  the pin answers no and rejects the call.
  - Cause: the shared rule answers the trait shallowly
    (`StructTraitRule::Shallow` in `mojito_types::conformance`, and the
    checker's `conforms_to` `_ => true` arm), as R577 describes for
    `Boolable`.
  - Unlike R577's traits, every fact is already at hand: a struct's
    `declared` rows, and `types::trivially_register_passable_leaf` for the
    compiler-known types upstream declares it on.
    `MirDeclarations::copy_is_identity` already answers this way below the
    waist.
  - Pinned by
    `conformance/probes/conforms_to_trivial_register_passable_undeclared.mojo`
    and `conformance/probes/trivial_register_passable_bound_undeclared.mojo`.
  - Probe: `cse` never merges a generator's result of a
    `T: TrivialRegisterPassable` type, since the bound admits a `List`
    instance whose consuming readers would share one buffer; once the
    bound holds, `MirDeclarations::copy_is_identity` can answer yes for
    such a `T`: `def both[T: TrivialRegisterPassable](x: T) -> Int:
    return pair(x, x)` keeps two `var.use` copies of `x` in the generator.
  - Probe: `mem-2-reg` keeps a slot of such a `T` for the same reason:
    `def keep[T: TrivialRegisterPassable](x: T) -> T: var y = x; return y`
    keeps `y`'s slot in the generator. Once the bound holds,
    `MirDeclarations::copy_is_identity` answers yes for a type parameter
    whose bound is or refines `TrivialRegisterPassable`, and the slot
    promotes (found while landing R1266).
  - Probe: `imm-arguments` leaves a `read x: T` of such a `T` in its
    parameter slot for the same reason, so each read of `x` in the
    generator is a separate `var.use`; once the bound holds it is read
    once at the entry, as a concrete `Int` is (found while landing
    R1076).
  - Found while landing R838 (2026-10-14).
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
  - Found while landing R838 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.
- [ ] **R861 A `Copyable` struct holding a `TrivialRegisterPassable` field
  is rejected as not `Copyable`**

  `struct Box(Copyable): var p: P`, with `P` declaring
  `TrivialRegisterPassable`, fails with "type 'Box' for parameter 'Self'
  does not conform to trait 'Copyable'", where the pin runs it.
  - `TrivialRegisterPassable` refines `ImplicitlyCopyable`, so the field is
    copyable; the synthesized `Copyable` row apparently misses the
    refinement.
  - The cause is not yet traced.
  - Pinned by
    `conformance/probes/copyable_struct_trivial_register_passable_field.mojo`;
    promote it to `assets/ok/` when it runs.
  - Found while landing R838 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.
- [ ] **R863 A `TrivialRegisterPassable` struct that does not declare
  `Copyable` cannot be copied explicitly**

  `q.copy()` fails the check with "type 'P' has no method 'copy'", and a
  generic `x.copy()` or `T(copy=x)` at `P` stops in the VM, where the pin
  runs all of them.
  - `TrivialRegisterPassable` refines `ImplicitlyCopyable`, which refines
    `Copyable`, so the struct's copy members should be synthesized as if
    `Copyable` were declared; declaring it makes every case run.
  - Likely the same missed refinement as R861.
  - Pinned by `conformance/probes/trivial_register_passable_copy_method.mojo`
    and `trivial_register_passable_generic_copy.mojo`; promote them to
    `assets/ok/` when they run.
  - Found while landing R839 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R578 Bundled library types declare other conformances than
  upstream's**

  Problem: `List[Int]` is neither `Equatable` nor `Defaultable` in Mojito,
  where the pin says it is both, and `String` is `Iterable` in Mojito but
  not at the pin.
  - Cause: the bundled `List` declares neither conformance, and the bundled
    `String` declares `Iterable` where upstream's does not.
  - A conformance is now decided from the declared list alone, so the
    declarations are the whole answer.
  - The fix is to declare each bundled type's conformances as the pin's
    library does, conditions included (`Equatable where T: Equatable`).
  - Pinned by
    `conformance/probes/conforms_to_library_declared_conformances.mojo`.
  - Found while landing R564 (2026-10-10).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R525 Materialization errors omit Mojo's contextual notes**

  The crossing diagnostic has one message where Mojo distinguishes a bare read from a call argument.
  - A bare read suggests `materialize`; a call argument suggests evaluating the whole call with `comptime`.
  - `TypeError` has no diagnostic-note channel.
  - Depends on nothing.
  - Model: Fable, Not Planned.

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

- [ ] **R579 `Representable` and `Ceilable` resolve with no import, where
  the pin needs one**

  Problem: `conforms_to(T, Representable)` and `conforms_to(T, Ceilable)`
  compile in Mojito with no import. The pin rejects the first as an unknown
  declaration and asks for `from std.math import Ceilable` for the second.
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

- [ ] **R980 A capturing callable parameter forwarded to a callee that
  calls it is rejected**

  Problem: `outer[f]` calling `inner[f](x)`, where `f` binds a non-generic
  capturing closure and `inner` calls `f`, is rejected at elaboration
  ("value parameter `f` is not compile-time constant"), while the pin runs
  it.
  - `outer`'s instance takes the closure as a runtime parameter
    (`Bindings::runtime_callables`), so the forwarded argument is no
    compile-time value `inner`'s instance can be keyed on.
  - A forward to a callee that removed the parameter works since R910.
  - Moving non-generic closures onto the environment path, which already
    forwards a generic closure's environment through another binding,
    should close it.
  - Found while landing R910 (2026-10-15).
  - Depends on R533.
  - Model: Opus, Not Planned.

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

- [ ] **R191 `Int` conforms to `Floatable`, though the pin does not**

  Problem: Mojito conforms `Int` and an integer literal to `Floatable`.
  Upstream conforms neither, so a `Floatable`-bounded helper takes only a
  float there.
  - A conformance leniency to withdraw.
  - Ledger name: `int-is-floatable`.
  - Depends on nothing.
  - Model: Fable, Not Planned.

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
  - Mojito formats a builtin scalar in the host and hands the text to
    `write_string` (the `$write_formatted` primitive), so the override is
    never re-entered.
  - Pinned by `conformance/probes/writer_override_scalar_write_to.mojo`.
  - Ledger name: `writer-override-scalar-write-to`.
  - Found while landing R414 (2026-10-06).
  - Depends on R276, which moves the scalar text into Mojo bodies.
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
  - Depends on nothing.
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

### Mojito-Specific Shortcuts To Move Toward Mojo's Shape

Track: `mojo-shape`.

Parts of Mojito's stdlib lean on the Rust runtime where upstream is pure Mojo,
or take a Mojito-specific shortcut where upstream has its own shape. Each entry
below is a candidate port toward Mojo's shape, and a port is preferred over any
new bridge (2026-09-07 direction). The ports are independent of each other, and
an entry that waits on another names it. Four runtime services are
deliberately not on this list; they are in [`docs/non-goals.md`](non-goals.md).

- [ ] **R1402 Seventeen `mojo-shape` entries wait on landed work, are done
  by their own test, or lack the Depends links their new cores need**

  Several entries cite code that is gone or wait on entries that have
  landed, two meet their own done condition, and the cores filed 10-25
  (R1357–R1399) were filed without editing their members' Depends bullets.
  - Cite gone code: R752 (`build_seeded_checked_region`, `RegionSeed`) and
    R346 (`infer_tuple_method`).
  - Wait on landed entries: R306 and R903 (R74), R800 (R801), R1169 (R1168),
    R445 (R10), R912 (R907), R1072 (R743, and the bundled `VariadicList`
    already declares `write_to` and `write_repr_to`), and R1287 (R1102).
  - R1335 asks for a census rerun after R1326, which has landed.
  - Done by their own test: R638 and R639. `rg 'ExprKind::TypeApply'
    crates/mojito-mir/src` and `rg 'ExprKind::Call|EvaluatedRange'
    crates/mojito-mir/src` find nothing.
  - Duplicate: R950 restates R662 (a value binder read through a slot). Fold
    R950's `mem-2-reg` `variance` bullet into R662.
  - Add `Depends on R1392 and R1395` to R336: upstream's `SIMDLength` is a
    struct over `index` ops whose arithmetic must fold in a parameter
    expression.
  - In R276, replace `R160` with `R1397`: `SIMD`'s text moves into Mojo with
    the library `SIMD`, which takes a `DType` parameter without R160.
  - For each entry, run its probe (or the program its first sentence names)
    on `target/debug/mojito` and on the pinned Mojo, through the Pixi
    project `docs/usage.md` names (`MOJO_PIXI_MANIFEST`).
  - Delete an entry whose probe now matches the pin, and record it in
    `CHANGELOG.md`.
  - Rewrite an entry that still fails so its cause names today's code, and
    keep its ID.
  - Merge a duplicate into the entry it restates, carrying its probes, and
    delete it.
  - Done when every entry listed here is deleted, rewritten against today's
    code, or merged.
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
  - A census of `emit-mir` over `assets/ok` and `assets/ownership_ok`
    (2026-10-24) finds one shape left: an owned `Error` read in a `try`
    handler no raise reaches, which the forward walk never enters, so
    drop elaboration records no facts there and places no `drop.reg`.
  - Probe: `assets/ok/pliron_reference_write_back.mojo`'s handler
    (`print(e)` over a body with no raising call), and
    `copyable_iterator_reference_aggregate`,
    `read_temporary_conditional_argument_drop`,
    `try_finally_raise_skips_own_handler`, and
    `try_region_shadowed_binding`.
  - Since R1326 (2026-10-24), `print(e)` reads `e` in place, so these
    probes' handlers hold no owned register; re-run the census before
    deciding whether the erasure below is still needed.
  - Fix: erase a handler no raise reaches before drop elaboration, as
    upstream's canonicalization of a `try` without one does, then report
    an owned register live at an exit.
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
    `numeric_operation_traits.mojo`).
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
  - Depends on R1371, which makes every bracket an index of expressions.
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

- [ ] **R705 `Equatable` and `Comparable` are compiler-known, where
  upstream's library declares them**

  Upstream declares both traits in `std/builtin/comparable.mojo`, with
  `__ne__` defaulted through `__eq__` and `__gt__`/`__le__`/`__ge__`
  through `__lt__`; Mojito knows them by name.
  - `compiler_trait_declaration` (`mojito-checked::declaration`) stands in
    for their requirements, and the checker's `has_equality_bound`,
    `has_order_bound`, and `NegatedEquality` stand in for the defaults.
  - This entry bundles the two declarations with their non-reflective
    defaults and deletes `compiler_trait_declaration`; upstream's
    reflective `__eq__` default stays out, waiting on R278 and R269.
  - Found while landing R702 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1360 `Boolable`, `Intable`, `Floatable`, `Indexer`, and
  `Identifiable` are compiler-known, where upstream's library declares
  them**

  Upstream declares them in `std/builtin/bool.mojo`, `int.mojo`,
  `floatable.mojo`, and `identifiable.mojo` (with a defaulted `__isnot__`)
  and exports them from the prelude. Mojito knows them by name, so no
  conformance records a witness table, and `StructTraitRule::Shallow`
  answers `Boolable` and `Identifiable` as held.
  - Bundle the declarations and drop the names from `COMPILER_TRAITS`. Move
    the scalar answers from `leaf_conforms` to `modelled_declares`, as
    `Sized` is answered.
  - Delete their arms in `builtin_trait_operation` and
    `verify_builtin_conformance`.
  - Probe: `conformance/probes/conforms_to_undeclared_compiler_trait.mojo`.
    Also, `conforms_to(P, Identifiable)` for a plain struct is `False` at
    the pin and `True` in Mojito.
  - Closes R577 (an undeclared `Boolable` is held) and R191 (`Int` is
    `Floatable`). Shrinks R1281 and R1109.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1361 The `std.math` operation traits are compiler-known, and a
  struct cannot conform to `Absable`, `Powable`, or `Roundable`**

  `struct A(Absable)` with `__abs__(self) -> Self` is rejected ("missing
  required operation '__abs__() -> Self'"), where the pin prints `3 True`
  for `abs(x)` through `T: Absable` and for `conforms_to(A, Absable)`.
  - Cause: `struct_trait_rule` answers these three as `Never`, and answers
    `Ceilable`, `Floorable`, `Truncable`, `CeilDivable`,
    `CeilDivableRaising`, and `DivModable` as `Shallow`. No bundled module
    declares them.
  - Upstream declares all nine in `std/math/math.mojo`. Only `Absable`,
    `Powable`, and `Roundable` are prelude names.
  - Declare them in `stdlib/std/math.mojo`, export the three from the
    prelude, and delete `math_dunder_bound` and their
    `builtin_trait_operation` rows.
  - Probe: `from std.math import Ceilable` fails in Mojito ("has no
    declaration named 'Ceilable'").
  - Closes R579's `Ceilable` half (resolves with no import).
  - Probe: `abs(x)`, `round(x)`, and `divmod(a, b)` over `x: T` lower as a
    `var.use copy` that a `drop.reg` destroys (census:
    `numeric_operation_traits.mojo`, `boolable_and_divmod.mojo`). Only
    scalars instantiate today, so nothing observes it; library functions
    with `read` parameters remove it.
  - Done when `Shallow` answers only `AnyType` and `Representable`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1362 `Hashable` and `Hasher` are compiler-known, where upstream's
  `std/hash` declares them**

  Upstream declares `Hashable` in `std/hash/hash.mojo` (prelude-exported,
  with a reflective default `__hash__`) and `Hasher` in `hasher.mojo` (not
  in the prelude). Mojito checks both by shape in
  `verify_builtin_conformance`, and their conformances record no witness, so
  a call through the bound keeps R1281's composed name.
  - Declare both with today's requirement sets. R25 then changes the bundled
    `Hasher` declaration, not checker code.
  - The checker-synthesized reflective default stays until R278 and R269 let
    the body read `reflect[Self]`, as R705 stages `__eq__`.
  - Every fixture spelling `Hasher` bare needs the pin's import.
  - Done when both leave `COMPILER_TRAITS` and `T: Hashable` at a struct
    resolves through its witness table.
  - Shrinks R25, R1281, R1109, and R987.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1363 `Defaultable`, `RegisterPassable`, and
  `TrivialRegisterPassable` are compiler-known, where upstream's
  `std/builtin/value.mojo` declares them**

  Upstream declares `TrivialRegisterPassable(Deinitable, ImplicitlyCopyable,
  Movable, RegisterPassable)`, and Mojito's `compiler_trait_refines` knows
  only its `RegisterPassable` parent, so a `TrivialRegisterPassable` struct
  is not `Copyable` and an undeclaring struct passes the bound.
  - Bundle the three in a new `stdlib/std/builtin/value.mojo`, exported from
    the prelude. Delete `compiler_trait_refines` and `Defaultable`'s row in
    `compiler_trait_declaration`.
  - Probe:
    `conformance/probes/copyable_struct_trivial_register_passable_field.mojo`,
    `trivial_register_passable_copy_method.mojo`, and
    `trivial_register_passable_bound_undeclared.mojo`.
  - Closes R861 and R863 (the missed refinement to `Copyable`) and R862 (an
    undeclared bound is accepted). Shrinks R860 and R1119 (its default body
    goes in this declaration).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1284 Compile-time `len` of a compile-time-only operand is a
  checker query, where upstream applies the library `len`**

  `len(Ts)` over a type pack, `len(tl)` over a `TypeList` binding,
  `len(Self.values)` over a raw list, and `len(names)` over a reflected
  list are answered by checker queries, where the pin applies its library
  `len` to the operand's value at compile time (its `Ts` is a `TypeList`
  value, whose `__len__` the `Sized` overload calls).
  - `call_inference::compile_time_len_operand` routes a type pack to the
    builtin `infer_len` ahead of the library lookup.
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
  - Found while landing R988 (2026-10-22).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R857 A built-in scalar conversion is a named builtin `call`, not a
  cast op**

  `Int(x)`, `UInt(x)`, `Float64(x)`, and `Bool(x)` stay a
  `call { func: Int|UInt|Float64|Bool }` that the VM, Pliron, and `sccp`
  each recognize by its bare name, where upstream inlines
  `SIMD.__init__` and `Bool.__init__` to a `pop.cast` or a compare.
  - A MIR cast instruction, lowered by the check, would carry the target
    type and need no name lookup, so a program's own `def UInt` could not
    be confused with it.
  - It touches MIR text and the verifier (a MIR version bump), the VM's
    `builtin_convert` dispatch, Pliron's `lower_convert`, and
    `MirInstr::scalar_conversion`, which would become the cast's fold.
  - Found while landing R856 (2026-10-14).
  - Depends on nothing.
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

- [ ] **R638 MIR lowers a `Variant` projection type application from
  syntax**

  `v[T]` as a `Variant` projection is a place, and MIR still finds it by
  matching `ExprKind::TypeApply` where it should match `CheckedOp::TypeApply`.
  - Sites: `expr_unconverted`'s `variant_project_type_apply` arm,
    `place_unrebound`, `simple_place_unrebound`, `try_place_unrebound`
    (`lower_stmt.rs`), `reference_handle` (`mir.rs`), and the
    `Member | Index | TypeApply` place predicate in `facts.rs`.
  - `call_family` already takes the callable `TypeApply` and returns `None`
    for a projection, so the projection arm is reached only from syntax.
  - Done when `rg 'ExprKind::TypeApply' crates/mojito-mir/src` finds only
    `expression_children` and the node-less boundary arm.
  - Found while landing R619 (2026-10-12).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R639 A `comptime for` range's unfolded bounds are read from the
  range call's syntax**

  `lower_term`'s `ComptimeSource::EvaluatedRange` arm matches the header's
  rebuilt `ExprKind::Call` to reach each bound it asks a thunk for, where
  the bound should come from the range call node's `args`.
  - The thunk request (`ComptimeThunks::request_value`) takes syntax, so
    the bound's node id must reach it, or the request rebuild the syntax
    itself.
  - Done when `lower_stmt.rs` names no `ExprKind::Call`.
  - Found while landing R619 (2026-10-12).
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

- [ ] **R664 Owned or empty-chain iteration normalizes its iterator into
  the source slot**

  `GetIter` with `source == dest` leaves `$iter<N>` typed by the iterable
  (`List[Int]`) while it holds the iterator (`_ListOwnedIter`).
  - Mono's `rewrite_get_iter` retypes the slot after the fact, from the
    resolved chain.
  - Upstream, `for x in l^` takes `l^.__iter__()` into its own iterator
    value.
  - The split path already types its slot by
    `IterationProtocol::iterator_ty`.
  - Done when every `GetIter` has its own `dest` slot typed by
    `iterator_ty`, and `rewrite_get_iter` writes no slot type.
  - Found while landing R661 (2026-10-13).
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
  - Found while planning R743 (2026-10-23).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1072 Writing a homogeneous collector is rejected**

  Problem: `print(xs)` over `*xs: Int` reports that the collector does not
  conform to trait 'Writable', where the pin prints `(1, 2, 3)`.
  - Upstream's `VariadicList` is `Writable` when its element is, through
    its `write_to` and `write_repr_to`.
  - The bundled `VariadicList` (`stdlib/std/builtin/variadics.mojo`)
    declares neither yet, nor its `Writable where ...` conformance.
  - Found while planning R1040 (2026-10-16).
  - Depends on R743.
  - Model: Opus, Not Planned.

- [ ] **R1315 A `!pop.array` size cannot be a parameter, so
  `_MLIR.POPArrayType` cannot be declared**

  Problem: MIR's `!pop.array` (`TyKind::PopArray`) holds a concrete size
  only, and the checker resolves no ``__mlir_type[`!pop.array<`, size, `,
  `, T, `>`]`` spelling, so the bundled `_MLIR` cannot declare upstream's
  `POPArrayType[size: __mlir_type.index, elt_type: AnyType]`.
  - Probe: adding upstream's `POPArrayType` member to `_MLIR`
    (`stdlib/std/builtin/variadics.mojo`) is rejected with "only ...
    '__mlir_type[`!kgen.param_list<`, T, `>`]' are accepted"
    (`checker/type_resolution.rs::mlir_param_list_type`).
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
  - `__mlir_type.index` already resolves to `Int`, so `__mlir_index__` and
    `Int(mlir_value=)` are identities here.
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

- [ ] **R662 A value binder read is a frame slot, not a parameter
  reference**

  `identifier_read` lowers a read of a scalar value binder (`n` in
  `def plain[n: Int]`) to a slot `Flatten::var` interns, which holds
  nothing until mono rewrites each read.
  - Upstream reads a parameter as an attribute (a decl-ref), with no
    storage.
  - The lever: lower the read as `param_value_register` lowers a recorded
    parameter value (`Const::Param`, answered by mono), from the checked
    binding the read names, as a lifted thunk's `parameter_reads` already
    does.
  - Done when a value binder's read takes no slot in MIR, and VM and
    `--backend pliron` output is unchanged.
  - Found while landing R660 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R663 Mono still rewrites value binder slots MIR no longer
  creates**

  Once binder reads are parameter references, the binder half of mono's
  slot machinery is dead weight.
  - `materialized_parameter_slots`' binder role,
    `substitute_value_parameter_reads`' binder half, and
    `seed_captured_parameter_slots` exist to serve the slot.
  - Done when they serve only what still takes a slot, or are deleted.
  - Found while landing R660 (2026-10-13).
  - Depends on R662.
  - Model: Opus, Not Planned.

- [ ] **R752 MIR re-enters HIR to build each `try` region's CFG**

  `Flatten::lower_region` calls `Cfg::build_seeded_checked_region` with
  MIR's own slot interner and binding map, where upstream lowers a `try`'s
  regions inside the one function walk.
  - The lever: HIR builds each sub-region's CFG while it walks the
    function, and `HirInstr::Try` carries them, so `RegionSeed` goes away.
  - MIR temporaries stop interleaving with region slots, so `VarId`
    numbering moves: review the MIR text of the `try`/`with` fixtures.
  - Done when MIR builds no HIR CFG and VM and `--backend pliron` output
    is unchanged.
  - Found while landing R636 (2026-10-13).
  - Depends on nothing.
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
    no `write_repr_to` for it, so `repr(Span(xs))` is the reflective
    default where the pin prints `Span[mut=True, SIMD[DType.int, 1]]([Int(1),
    Int(2)])`.
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
  - This also closes R521.
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

  Problem: `range(Int32(4))` is typed by `Checker::infer_scalar_range`
  once overload selection fails, which spells the call as the range
  struct's construction, where the pin's `std.builtin.range`
  declares `def range[dtype: DType, //](end: Scalar[dtype]) ->
  _ZeroStartingRange[dtype]` and its two- and three-argument siblings.
  - Mojito's float range is a separate `_FloatStridedRange`; the pin's
    `_StridedRange[dtype, forward]` takes the float path under `comptime if
    Self.dtype.is_floating_point()`.
  - The overloads reject a float or `Bool` lane with `comptime assert`.
  - With the overloads in source, `infer_scalar_range`'s spelled
    construction and the checker's range shortcuts (`scalar_range_parts`
    in `iteration.rs`, `indexing.rs`, and `builtins.rs`) go.
  - Depends on R74.
  - Model: Opus, Planned.

- [ ] **R903 A `comptime for` over a display of non-`Copyable` elements is
  rejected with Mojito's wording, not the pin's**

  The bundled `Array.__iter__(ref self)` states its requirement as `where
  conforms_to(Self.T, Copyable)`, where upstream uses a `comptime assert`.
  - A non-`Copyable` element therefore selects `__iter__(var self)`, and the
    `paramfor_*` bound rejects `_ArrayOwnedIter[P]` as not `Copyable`.
  - The pin reports "Array iteration requires the element to be
    `Copyable`" from `array.mojo:1153`.
  - Fixture: `assets/type_error/comptime_for_display_not_copyable.mojo`.
  - Found while landing R794 (2026-10-14).
  - Depends on R74.
  - Model: Opus, Not Planned.

- [ ] **R904 A `comptime for` over a type pack or a reflected field-type
  list fails with Mojito's own wording**

  Problem: the pin rejects both loops because a `TypeList` has no
  `__iter__`, while Mojito fails somewhere else with its own message.
  - `comptime for T in reflect[P].field_types()` gives "not a compile-time
    value: type-valued or symbolic comptime values cannot materialize at
    runtime"; the pin says "'TypeList[Int, String]' does not implement the
    '__iter__' method".
  - `comptime for T in Ts` over `*Ts: AnyType` gives "Undefined variable
    'Ts'"; the pin says "'TypeList[Ts.values]' does not implement the
    '__iter__' method".
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
  - Fix: bind the prelude's names in bootstrap modules too, then drop the
    six imports.
  - Found while landing R791 (2026-10-14).
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

  Problem: `comptime e = vs[1]`, `Box[vs[0]]`, and `materialize[vs[i]]()`
  are compiled by the checker straight to a `ListGet` over the pack, where
  upstream reads them through `ParameterList.__getitem_param__`.
  - The checker site is the `Index` arm of `compile_dependent_ct_expr`
    (`checker/constraints.rs`), which also serves display lists and module
    display constants and must keep doing so.
  - Add `comptime __getitem_param__[idx: Int]: Self.type = Self.values[idx]`
    to `stdlib/std/builtin/variadics.mojo`, read under the raw-list gate
    (`Checker::bundled_raw_list_operand`), as `size` already is.
  - Upstream's index parameter is `idx: SIMDLength`, which Mojito lacks, so
    the port spells it `Int` and records that in the module header.
  - A runtime subscript already calls `__getitem__`, and the length is
    already `size`.
  - Probe: over `struct R[t: AnyType, //, values:
    _MLIR.KGENParamListType[t]]`, `comptime first: Int = Self.values[0]`
    prints `4` for `R[vs.values]` with `vs = 4, 5`, where the pin rejects
    it with "'KGENParamList[t]' is not subscriptable". The same arm must
    take a raw list only under the bundled gate (found while landing
    R1168, 2026-10-20).
  - Found while landing R494 (2026-10-13).
  - Depends on R801, which gives a `comptime` value member parameters.
  - Model: Fable, Planned.

- [ ] **R1169 A type pack is not a `TypeList` value**

  Problem: `var x = Ts` and `comptime tl: TypeList[Trait=Movable,
  Ts.values] = Ts` stop with "Undefined variable 'Ts'", where the pin runs
  them, and `len(Ts)` is a checker rule rather
  than `TypeList.__len__`.
  - Read the pack in a runtime position as `TypeList[Ts.values]()`, as
    `infer_value_pack_read` reads a value pack as
    `ParameterList[vs.values]()`.
  - `len(Ts)` then dispatches to `__len__` like any `Sized`; delete
    `infer_len`'s pack arm and `pack_length_binder`'s `len` arm.
  - `comptime y: Int = Ts` should then report the pin's "cannot implicitly
    convert 'TypeList[Ts.values]' value to 'Int'", not "Undefined variable
    'y'".
  - Found while planning R805 (2026-10-19).
  - Depends on R1168.
  - Model: Fable, Planned.

- [ ] **R1182 `TypeList`'s list vocabulary is answered by spelling, not by
  the bundled struct's members**

  Problem: `reverse`, `slice`, `map`, `filter_idx`, `reduce`, `any`,
  `all`, `all_conforms_to`, and `contains` are compiler rules keyed on the
  name `TypeList`, where upstream declares each on the struct.
  - The checker answers them in `Checker::typelist_receiver` and
    `compile_typelist_proposition` (`checker/constraints.rs`).
  - The pre-check elaborator folds them by spelling in
    `comptime/rewrite.rs:fold_pack_typelist_use` and `comptime/eval.rs`.
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

- [ ] **R389 One compile-time expression is lifted as more than one
  function**

  Problem: `comptime if h(n) > 4:` and `flag[h(n) > 4]()` in `def f[n:
  Int]()` evaluate two functions for the one expression, where the pin holds
  one parameter expression and every read names it.
  - The check names the function of an expression in a type or a parameter
    argument, or bound by a local `comptime` (`Checker::lifted_application`,
    `checker/comptime_validation.rs`).
  - MIR names its own for a condition, a `range` bound, and a runtime read
    of a display (`ComptimeThunks::request_value`, `mir.rs`).
  - An operand the check lifted before the whole expression failed to
    compile keeps its function too: `flag[h(vals[0]) > 2]()` lifts
    `h(vals[0])` and the comparison.
  - The values agree, and an unread function is never instantiated. Only
    the source MIR and the elaborator's work grow.
  - Let the check name every lifted expression, and drop a function nothing
    applies.
  - Depends on nothing.
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

- [ ] **R1188 `Bool` is a compiler-modelled scalar where upstream's is a
  library struct**

  Problem: `True.__int__()` fails with "type 'Bool' has no method
  '__int__'", where the pin runs it, because Mojito answers `Bool`'s
  members by compiler rules rather than upstream's `bool.mojo`
  declarations.
  - `Bool` is `TyKind::Bool`, and its operators, conversions, and traits
    are checker, VM, and native rules. There is no `struct Bool` under
    `stdlib/`.
  - Port it as a library struct over its `i1` storage, the way R1157 and
    R1158 port the slices.
  - The planning session splits it into chained one-session entries if
    one session cannot land it.
  - Found while planning R1132 (2026-10-20).
  - Depends on nothing.
  - Model: Fable, Planned.

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
  - Depends on R1132 (the trait) and R1188 (a library `Bool`).
  - Model: Opus, Not Planned.

- [ ] **R1192 `StringLiteral` is a compiler-modelled type where upstream's
  is `StringLiteral[value: !kgen.string]`**

  Problem: `print(StringLiteral["None".value]())` fails with "Undefined
  variable 'StringLiteral'", where the pin prints `None`, because Mojito's
  string literal type has no `value` parameter.
  - `StringLiteral` is `TyKind::StringLiteral`, and a literal's `.value` is
    a compiler fold (`CtValue::literal_value`) rather than a read of the
    type's parameter.
  - Port upstream's `std/builtin/string_literal.mojo` struct, the way R1188
    ports `Bool`, so a literal's type is the application its text keys.
  - Probe: `StringLiteral[v]().byte_length()` over `comptime v =
    "None".value`, which prints `4` at the pin.
  - Probe: a symbolic read `def f[s: StringLiteral](): g[s.value]()`,
    which the pin runs, fails with "unsupported associated comptime member
    access".
  - Converting a KGEN string to a runtime string (`StaticString(names[i])`)
    then follows with R432.
  - The planning session splits it into chained one-session entries if
    one session cannot land it.
  - Found while landing R1183 (2026-10-20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1389 `Int`, `UInt`, and `Float64` are type kinds of their own,
  where upstream's are `Scalar[DType.int]`, `Scalar[DType.uint]`, and
  `Scalar[DType.float64]`**

  Mojito has `TyKind::Int`, `UInt`, and `Float64` beside `TyKind::Simd`,
  where upstream's `std/simd.mojo` declares all three as aliases of the one
  `SIMD` struct.
  - `canonical_simd_ty` turns width one back into the native kinds, and
    `simd_shape` reports `UInt` as `uint64`. No `Dtype` stands for
    upstream's `DType.uint`.
  - The lever: make the three a width-one `TyKind::Simd` that still displays
    its alias name, and add `DType.uint`. The VM's `Value::Int`, `UInt`, and
    `Float64` stay as the width-one representation.
  - Splitting `Int` from `SIMD` would be a Mojito-only split, so this merge
    comes first and one library port (`SIMD`) then covers every scalar.
  - Probe: `g[dt: DType](a: Scalar[dt])` called with `UInt(3)`, and `var x:
    Scalar[DType.uint] = UInt(3)`. Mojito rejects both ("not a valid SIMD
    element type: DType.uint"). The pin prints `6` and `3`.
  - Shrinks R335 and R143.
  - Done when `canonical_simd_ty` has no special arm.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1390 The VM and Pliron serve no lane method on an `Int`, `UInt`, or
  `Float64` value**

  Once the checker types `Float64(2.5).reduce_add()` and `x.ne(y)` on a
  width-one native scalar, `runtime::simd_method` and Pliron's SIMD method
  lowering still skip `Value::Int`, `UInt`, and `Float64`, where upstream's
  are ordinary `SIMD` methods.
  - Serve the width-one native values in both backends. For a NaN, `ne`
    stays ordered (`s.ne(s)` is `False`).
  - Probe: R335's `total(Float64(2.5))` and R143's `x.ne(y)`.
  - Closes R335 (lane reductions on native scalars) and R143 (scalar
    comparison methods).
  - Depends on R1389.
  - Model: Opus, Not Planned.

- [ ] **R1391 `__mlir_type` cannot spell `!kgen.simd`, so `SIMD`'s storage
  field cannot be declared**

  ``__mlir_type[`!kgen.simd<`, Self.length._mlir_value, `, `,
  Self.dtype._mlir_value, `>`]``, the type of upstream's `SIMD._mlir_value`,
  is rejected by the accepted-spellings list in `checker/type_resolution.rs`
  (`mlir_param_list_type`).
  - Resolve it to `TyKind::Simd`, whose known and expression slots already
    have this shape. Share the slot resolver R1315 needs for `!pop.array`.
  - Read `_mlir_value` of a `DType` or width parameter as the parameter
    itself until R160 and R336 land, as `__mlir_index__` is read today.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1392 `__mlir_op` admits no `pop` or `index` op, so no `SIMD`
  operator body can be written**

  `checker/method_calls/mlir_op.rs` accepts only the two ownership markers,
  where upstream's `SIMD`, `SIMDLength`, and `Bool` bodies are made of `pop`
  and `index` ops.
  - The ops: `pop.add`, `sub`, `mul`, `div`, `floordiv`, `rem`, `neg`,
    `abs`, `shl`, `shr`, `pop.cmp`, `pop.cast`, `pop.simd.splat`,
    `pop.simd.and`, `or`, `xor`, the rounding ops, `fma`, and `index.*`.
  - Type each one over `!kgen.simd` operands, and lower them to the existing
    `BinOp`, `UnOp`, and `MakeSimd` and to R857's cast instruction.
  - Accept `__mlir_attr` for `#kgen.cmp_pred<...>` and `#pop.fmf<...>`.
  - Keep the forms bundled-only, as the ownership markers are.
  - Compute float `floordiv` and `rem` in the VM, R850's backend half.
  - Depends on R1391 (the operand type) and R857 (the cast instruction
    `pop.cast` lowers to).
  - Model: Fable, Not Planned.

- [ ] **R1393 `SIMD`'s lane ops are host methods, where upstream's are
  `pop.simd` ops**

  `select`, lane extract and insert, and `reduce_and`/`reduce_or` run in
  `runtime::simd_method` and Pliron's per-method code, where upstream's
  bodies call `pop.simd.select`, `extractelement`, `insertelement`,
  `reduce_and`, `reduce_or`, and `shuffle`.
  - Add one MIR instruction for each, next to `SimdShuffle`, and teach the
    VM, Pliron, and `__mlir_op` typing.
  - Depends on R1392.
  - Model: Opus, Not Planned.

- [ ] **R1394 `llvm_intrinsic` is undefined**

  Upstream's `SIMD` reaches `llvm.vector.reduce.*`, `fma`, and the bit ops
  through `std.sys.intrinsics.llvm_intrinsic[name, T]`
  (`pop.call_llvm_intrinsic`), and Mojito has no such call.
  - Lower it to a MIR call that names the intrinsic. Pliron emits the LLVM
    call. The VM serves a table by name and rejects an unknown name
    explicitly.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1395 A call of an `@always_inline("builtin")` function is not
  folded in a parameter expression**

  `SIMD[DType.int8, twice(n)]` and `SIMD[DType.int8, n + n]` are different
  types to Mojito ("expected SIMD[DType.int8, 2 * n], found SIMD[DType.int8,
  twice(n)]"), where the pin folds the builtin body
  (`SharedState::foldInlineBuiltinFunction`, called from
  `UncheckedCallEmission.cpp`).
  - `InlineLevel::Builtin` is read only by `inline_parametric` today.
  - The lever: in `Checker::lifted_application`, substitute parameter
    operands into the body's `pop` and `index` ops and fold the result to a
    `ParamExpr`. Reject a body that does not fold, as upstream does.
  - Every type-level `n + 1` becomes such a call once `Int` and `SIMDLength`
    arithmetic are library code, so this lands before them. It also gives
    R851's "could not simplify operator shr" diagnostic a home.
  - Probe: `f[n](x: SIMD[DType.int8, twice(n)]) -> SIMD[DType.int8, n + n]`
    over the `twice` above prints `[1, 1, 1, 1]` at the pin and is rejected
    by Mojito.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1396 A one-field register-passable struct is a boxed struct value
  in the VM**

  Every struct is a `Value::Struct` with a name and a field vector
  (`runtime.rs`), so a library `SIMD`, and through R1389 every `Int`, would
  allocate per value, where upstream's `TrivialRegisterPassable` struct
  reaches LLVM as its element after `SROA`.
  - After builtin inlining, `canonicalize` forwards a `GetField` of a
    `MakeStruct`.
  - A register value of a `TrivialRegisterPassable` struct whose one field
    is register-passable is held as that field.
  - Done when the concrete MIR of an inlined `@always_inline("builtin")`
    `__add__` on a one-field struct holds no `MakeStruct`.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1397 `SIMD` is a compiler type, where upstream's is `struct
  SIMD[dtype: DType, length: SIMDLength]` over `!kgen.simd`**

  The name `SIMD`, and through R1389 `Int`, `UInt`, `Float64`, and the sized
  aliases, resolves to `TyKind::Simd` with checker-rule members, where
  upstream declares the struct and its aliases in `std/simd.mojo`.
  - Bundle `stdlib/std/simd.mojo`. Port upstream's bodies for every member
    the checker serves today, plus `__len__`, `__getitem__`, `__int__`,
    `__floordiv__`, and `__mod__`. `TyKind::Simd` stays only as the storage
    type.
  - A literal still converts by the checker's coercion. Upstream's
    `#pop.int_literal_convert` needs a parametric `IntLiteral[value]`, a
    residue to file.
  - This is the one step that cannot be split. The entries before and after
    it take everything that can be.
  - Closes R330 (lane gathers become library methods), R331 (shuffle's `2 *
    length` bound), R1223 (`__len__` and `__getitem__`), R187 (upstream's
    reduction tree), and R850 (float `//` and `%`). Shrinks R212 and R171.
  - Depends on R1390, R1392, R1393, R1394, R1395, R1396, R336 (the length
    type), and R185 and R186 (so the port is upstream's bodies with no
    leniency).
  - Model: Fable, Planned.

- [ ] **R1398 The checker, VM, and Pliron keep `SIMD` member rules no
  program reaches**

  After R1397, `checker/method_calls/simd_receivers.rs`, the SIMD arms of
  `checker/operators.rs`, `LaneMask::resolve`, `runtime::simd_method`, and
  Pliron's per-method SIMD lowering serve only the storage type, which has
  no members upstream.
  - Delete them.
  - Done when `rg simd_method crates` finds nothing.
  - Depends on R1397.
  - Model: Opus, Not Planned.

- [ ] **R1399 `Int(x)`, `UInt(x)`, and `Float64(x)` are checker built-ins,
  where upstream's are `SIMD.__init__` overloads**

  `checker/builtins.rs` types the conversions by rule, and the VM's
  `builtin_convert` and Pliron's `lower_convert` run them, where upstream
  declares `__init__[T: Intable]`, `__init__[T: IntableRaising](out self:
  Int, ...) raises`, and `__init__[T: Floatable](out self: Float64, ...)`.
  - Port the overloads, including the refined `out self` they use, and
    delete the scalar arms of all three sites. R857's cast becomes their
    `pop.cast`.
  - Probe: R430's `Int(StringSpan("12"))` and R339's `__int__`.
  - Closes R339 (`__int__` on a scalar) and R430 (`Int` of a `StringSpan`).
  - Depends on R1397.
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
  - Depends on R424 (`Writable` declared in `std/format`).
  - Model: Opus, Not Planned.

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
  - Probe: `x.write_repr_to(w)` on an `Int`, `UInt`, or `Float64`
    (`SemanticAdjustment::InvertedReprWrite`, `mir/lower_expr/expr_method.rs`)
    builds `repr(x)` into an owned `String` and writes that, where
    upstream's `SIMD.write_repr_to` writes into the writer itself.
  - Probe: `Int8(4).write_repr_to(t)`, `Float32(1.5).write_repr_to(t)`, and
    `Int8(4).write_to(t)` stop with "type 'Int8' has no method
    'write_repr_to'" (or `'write_to'`), where the pin writes `Int8(4)` and
    `Float32(1.5)`: a sized scalar's host method table has neither.
  - Depends on R160.
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
  - A self-hosted `SIMD` would move the typing, the constraints, and the
    lowering choice into source, and `LaneMask` would shrink to the known
    mask.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R336 `SIMDLength` is `Int` in Mojito, told apart only where a width
  binder is inferred**

  Problem: upstream declares `SIMD[dtype: DType, length: SIMDLength]`, so a
  vector argument solves a `w: SIMDLength` binder and leaves an `n: Int` one
  unresolved; Mojito types both as `Int` and records the `SIMDLength`
  binders in a checker side table (`Checker::simd_length_binders`) that
  only the solver reads.
  - Behavior matches the pin today: `SIMD[_, _]`'s width binder, a declared
    `w: SIMDLength`, and the `H: Hasher` requirement's width solve, and an
    `Int` binder in the slot is rejected
    (`assets/type_error/simd_int_width_binder_not_inferred.mojo`).
  - Upstream's shape is a distinct `SIMDLength` type with an implicit
    conversion from `Int`, so the declaration, not a side table, carries it.
  - Found while landing R259 (2026-10-05).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R346 The discovery check types `Tuple`'s dunders and teardowns in
  Rust though `tuple.mojo` declares them**

  Problem: `infer_tuple_method`
  (`checker/method_calls/builtin_types.rs`) still types `__len__`,
  `__contains__`, the six comparisons, `consume_elements`, and `deinit_with`
  from the element list for a `Tuple` no specialization serves yet.
  - `reverse` and `concat` are typed against the declaration's shell
    (`infer_tuple_member`), and the same path can answer the rest.
  - The Rust arms answer first, so removing one moves its member to the
    declaration.
  - Depends on nothing.
  - Model: Opus, Planned.

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

- [ ] **R424 `Writable` is a compiler builtin where upstream declares it in
  `std/format`**

  Problem: `Writable` and its `write_to`/`write_repr_to` requirements are
  synthesized in the checker (`builtin_requirements`, `checker/traits.rs`),
  where upstream's `std/format/__init__.mojo` declares the trait beside
  `Writer`, with Mojo default bodies.
  - The reflective `Name(field=value, ...)` default the checker synthesizes
    for a conformer without a `write_to` is upstream's default body.
  - `Writer` moved to the bundled `std/format/__init__.mojo` with R414, so
    this is the remaining half of that module.
  - Found while landing R414 (2026-10-06).
  - Probe: `repr(P[Int](1))` over `struct P[T: AnyType](Writable)` with
    only a `write_to` prints the elaborated instance name
    `P$mono$TInt(x=Int(1))`, where upstream's default body prints
    `P[SIMD[DType.int, 1]](x=Int(1))`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1287 `print` is a compiler builtin where upstream's is an
  `@inline(.never)` library function**

  `print` is checked, run, and lowered as a builtin call (`infer_print`,
  the VM's `dispatch.rs`, Pliron's `lower/calls.rs`), where upstream's
  `std/io/io.mojo` declares `def print[*Ts: Writable](*values: *Ts, sep:
  StringSlice = " ", end: StringSlice = "\n", flush: Bool = False, var
  file: RawFileDescriptor = stdout)`.
  - `inline_cost` prices the builtin call at the figure the pin's
    `print(s)` call site costs (`PRINT_COST`, 30), since the call holds
    none of the pack and defaults the pin's call site builds.
  - Probe: `@always_inline def show(x: Int): print("show", x)` inlines
    here and keeps its call at the pin, where a `StringLiteral` value
    takes `print("a")` past the bound.
  - The pin prices `print()` at 24 and `print(s, x)` at 31.
  - Done when `print` is the library function, the caller's MIR holds its
    pack and defaults, and `PRINT_COST` is gone.
  - Probe: a spread into the builtin `print` (`print(*args)`) is passed as
    its elements (`expand_pack_spreads`), where every other call passes
    the caller's pack itself.
  - Found while landing R1075 (2026-10-22).
  - Depends on R424, R422, R1102.
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

  Problem: `T()` and `Ts[i]()` are carried by `MirInstr::ConstructTypeParam`
  (a binder, plus a pack index), while `types[i]()` is carried by
  `MirInstr::ConstructType` over the type expression, where the pin has one
  operation: the `Defaultable` initializer of a type-valued parameter
  expression.
  - Folding the nullary forms gives `T()` the operand `Ty::Param` and
    `Ts[i]()` the dependent selection, closed by the elaborator's one
    `ConstructType` arm (`mono::substitute::default_construct_parameters`).
  - The erased VM constructs those two forms from type reification by slot
    name, so the fold waits for the erased oracle's removal.
  - `T(copy=x)` keeps its own form, or becomes a selected `Copyable`
    initializer call.
  - Probe: `inline-parametric` keeps an `each[Int, Bool]()` call over
    `comptime for i in range(len(Ts)): print(Ts[i]())`: the inlined
    `Ts[i]()` would name the callee's pack binder, which the caller does
    not declare, though substitution already closes its result type to
    the bound list's element at `i`, which a `ConstructType` would carry.
  - Found while landing R364 (2026-10-07).
  - Depends on R10, which deletes the erased oracle's reification.
  - Model: Opus, Not Planned.

- [ ] **R533 A non-generic capturing closure bound to a callable parameter
  is passed whole and called indirectly**

  Problem: a generic capturing closure's instance folds the closure's body
  and takes only its environment, but a non-generic one is still promoted
  to a runtime closure parameter (`Bindings::runtime_callables`), where the
  pin specializes on the bound function in both cases.
  - Two mechanisms carry one rule (`mono/promote.rs`).
  - Moving the non-generic case onto the environment path leaves one, and
    makes its calls direct.
  - A body using the parameter as a value would rebuild a `MakeClosure`
    over the environment parameters.
  - Found while landing R526 (2026-10-09).
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
    its body never reads.
  - Moving to upstream's shape rewrites the declaration's `param_decls`
    and every `param_arg` and `instantiated_args` naming the function,
    which needs the whole program at once, as R907 does.
  - Depends on R907.
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

- [ ] **R950 A compile-time value parameter is read through a slot named
  by its spelling**

  A generic body reads a value binder `k` as a `var.use` of a slot named
  `k`, where upstream reads `kgen.param.constant` of the binder's
  declaration reference.
  - The elaborator folds the reads by spelling
    (`substitute_value_parameter_reads`), and `inline-parametric` does the
    same for an inlined callee (`read_binder`).
  - Lowering should emit `Const::Param` of the binder's declaration
    reference, and both name-keyed rewrites then go away.
  - `mem-2-reg` refuses a slot nothing stores or declares for this reason
    (`variance`): its reads are binder reads, and that rule goes with the
    slot. Probe: `n` in `$comptime$unrolled$0` of a `comptime for k in
    range(n)` body.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1151 A variadic value parameter is typed by its element, not by
  upstream's raw list**

  Problem: upstream's `*values: T` binder is a `!kgen.param_list<T>` with
  variadic binding sugar, where Mojito declares it `ParamDecl::Value { ty:
  T, variadic: true }` and re-derives the list in
  `ParamDecl::binder_meta`.
  - A raw-list binder (`values: KGENParamList[T]`) and a pack therefore
    share a meta-type but not a declared type, so the checker keeps a
    second scope to tell them apart (`Checker::value_packs`,
    `value_pack_in_scope`).
  - Every reader of a variadic declaration's `ty` moves with the change:
    `value_pack_scope`, `self_value_pack`, the variadic arm of
    `resolve_use_params`, `bound_value_ty`, and the MIR verifier's pack
    checks.
  - A session should plan this first and split it if it does not fit.
  - Found while landing R807 (2026-10-19).
  - Depends on nothing.
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

- [ ] **R1405 Nine `stdlib` entries are fixed or wait on landed work**

  Three `stdlib` entries already run as at the pin, and the rest predate the
  work they wait on.
  - Fixed, by probes run 10-25: R213 (`"abc".byte_length()` prints 3), R216
    (`isdir("/tmp")` runs), and R220 (`Array[Int8, 4]()` runs).
  - Partly fixed: R1223. `len(5)` prints 1. `n.__getitem__(0)` and `SIMD`
    destructuring still fail, and R1397 closes them.
  - Cite gone code: R206 (`NestedMono`, `def_specialization_requests`,
    `replace_templates`).
  - Wait on landed entries: R199 (R40) and R411 (R74).
  - Overtaken: R204 by R1052, which folds a witness at substitution, though
    `TRAIT_DISPATCH_PREFIX` remains. R1101's `Writer.write` probe can run
    now that R1079 has landed.
  - For each entry, run its probe (or the program its first sentence names)
    on `target/debug/mojito` and on the pinned Mojo, through the Pixi
    project `docs/usage.md` names (`MOJO_PIXI_MANIFEST`).
  - Delete an entry whose probe now matches the pin, and record it in
    `CHANGELOG.md`.
  - Rewrite an entry that still fails so its cause names today's code, and
    keep its ID.
  - Merge a duplicate into the entry it restates, carrying its probes, and
    delete it.
  - Done when every entry listed here is deleted, rewritten against today's
    code, or merged.
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
    stand for `Defaultable` (`checker/call_inference.rs`,
    `method_calls/type_receivers.rs`) under the bundled
    `hash[HasherType: Hasher]`, where the pin rejects it. Since R1283
    names the `Defaultable` witness at `HasherType()`, it fails at
    elaboration ("constructs the type `SumHasher` … after
    monomorphization") rather than in the check.
  - Found at the 2026-10-17 re-pin (Mojo 1.2.0.dev2026101705).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1223 `Int`, `UInt`, `Float64`, and `SIMD` have no `__len__` or
  `__getitem__`**

  Problem: `len(n)`, `n.__getitem__(0)`, and `var [a] = n` reject for an
  `Int` (`type 'Int' has no method '__len__'`), where the pin's scalars
  have both, returning 1 and the value, and `SIMD` returns its width and
  a lane: `var [a, b] = SIMD[DType.int32, 2](1, 2)` prints `1 2`.
  - `modelled_declares` already calls these types `Sized`, so without the
    methods R1136's sequence destructuring rejects them explicitly
    ("sequence destructuring of 'Int', which has no
    '__len__'/'__getitem__' yet" in `checker/destructure.rs`); lift that
    rejection when the methods land.
  - A sequence *pattern* over these types crashes the pin, so
    `compile_sequence_pattern`'s rejection stays.
  - Stop: `len(5)`, `var [a] = 5`, the `SIMD` destructuring above, and
    `var [a, b] = 5` raising the length error all match the pin.
  - Found while planning R1136 (2026-10-20).
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
  - Watch R862: Mojito's `TrivialRegisterPassable` is not yet a checked
    conformance.
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
    'self'"), so the overload does not parse.
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

- [ ] **R1101 `String.__init__(literal)` carries no inline marking in the
  bundled library**

  The pin marks `String.__init__(out self, data: StringLiteral, /)`
  `@always_inline("nodebug")`, and the bundled `stdlib/std/string.mojo`
  leaves its twin unmarked where its neighbours carry `@inline(.nodebug)`,
  so `String("hi")` keeps its call where every other `nodebug` constructor
  inlines.
  - R786 imported the pin's markings in the pin's spelling; this one was
    missed, and any other `@always_inline("nodebug")` spelling the import
    skipped should be checked the same way.
  - Probe: `Writer.write[*Ts]` (`std/format/__init__.mojo`) and
    `String.write[*Ts]` (`std/string.mojo`) carry no marking; if the pin
    marks them, `print`'s variadic wrappers inline now that collectors do
    (R1079).
  - Done when the marking is on, `assets/ok/always_inline_nodebug_calls.mojo`
    prints as before, and `String("hi")` in `main` holds no `__init__`
    call.
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
  - Depends on R808 (generator parameters) and R800 (`__getitem_param__`).
  - Model: Opus, Not Planned.

- [ ] **R811 `ParameterList.tabulate`, `splat`, and `_concat` are not
  ported**

  Problem: the pin's `ParameterList` builds lists with these members, and
  the bundled one has none.
  - Upstream uses `#kgen.param_list.tabulate` and `.concat`. Port them as
    variadics-private stand-ins, like R810's.
  - Found while planning R493 (2026-10-13).
  - Depends on R807 (the raw list type) and R808 (generator parameters).
  - Model: Opus, Not Planned.

- [ ] **R812 `ParameterList.map_to_type` is not ported**

  Problem: the pin's `ParameterList.map_to_type` is missing from the
  bundled struct.
  - Its body is `TypeList.tabulate[...]`, so it needs a bundled `TypeList`
    with `tabulate`.
  - Found while planning R493 (2026-10-13).
  - Depends on R811 and R1181 (`TypeList.tabulate`).
  - Model: Opus, Not Planned.

- [ ] **R1181 `TypeList.tabulate`, `splat`, and `_concat` are not ported**

  Problem: the bundled `TypeList` has no list builders, where the pin's
  builds lists with these members.
  - `TypeList._concat[A.values, B.values]()` is a compiler rule today
    (`Checker::typelist_receiver`, and `comptime/rewrite.rs` for a bound
    pack).
  - Upstream uses `#kgen.param_list.tabulate` and `.concat`. Port them as
    variadics-private stand-ins, as R811 does for `ParameterList`.
  - Found while landing R1168 (2026-10-20).
  - Depends on R808 (generator parameters).
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

- [ ] **R204 An abstract dispatch target that survives elaboration aborts at
  run time**

  Problem: a call the checker emits as an abstract `__trait_dispatch.*` or
  `__iterator_dispatch.*` target that `native::mono` leaves unresolved
  reaches the VM, whose by-name retargeting then aborts, where the
  elaborator should reject the program.
  - The VM keeps the retargeting (`TRAIT_DISPATCH_PREFIX`,
    `symbol::borrowed_iterator_dispatch_alternate`; `backend/vm/exec.rs`,
    `invoke.rs`) only for such a survivor; a target the elaborator resolves
    (`mono/infer.rs`) never reaches it.
  - Sites: an operator or protocol dunder reached from a value whose static
    type is a bare `Ty::Param` the instance did not close; an instance
    reached only from bundled code.
  - Each survivor should be a `MonoError` at the call that reaches it, as the
    pin rejects an unsatisfied requirement; the retargeting goes with the
    last one.
  - Depends on nothing.
  - Model: Fable, Planned.

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
  - The pre-check elaborator's `Elab::type_from_name` (`comptime/elab.rs`)
    applies any other variadic struct at fixed arity, so over `struct
    S[*Ts: Movable]` a `comptime F = S[Int, Bool].n` stops with "type 'S'
    expects 1 compile-time argument(s), got 2", and `materialize[S[Int,
    Bool].A == Int]()` with "a parameterized type is not a value", where
    the pin runs both (found while landing R805, 2026-10-20).
  - Probe: a module alias of a variadic struct application, `comptime
    Alias = W[Bool, Int, String]` over `struct W[*Ts: AnyType]`, stops
    with "type 'W' expects 1 compile-time argument(s), got 3" at
    `Alias.n`, where the pin reads the member (found while landing R1202,
    2026-10-20).
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
  - Depends on R341 (the storage model and `unsafe_write_move_from`), R134
    (`type_of`), R340 (`Self.__len__()` in a header), and R344.
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

- [ ] **R339 A scalar has no `__int__` method**

  Problem: `v.reduce_add().__int__()` on a `SIMD[DType.int32, n]` prints
  at the pin, while Mojito reports "type 'Int32' has no method '__int__'".
  - `Int(x)` converts the same scalar, through the conversion built-in,
    not a method (`checker/builtins.rs`).
  - Upstream's scalar is a width-1 `SIMD`, whose `Intable` conformance is
    an ordinary method.
  - Found while landing R4's struct-valued half (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R335 The native `Int` and `Float64` have no lane reductions, even as a
  `Scalar[dt]` instance**

  Problem: `v.reduce_add()` in `def total[dt: DType](v: Scalar[dt])` fails at
  run time with "vm backend does not support methods on Float64 yet" for
  `total(Float64(2.5))` (and on `Int` for `total(Int(3))`), and
  `Float64(2.5).reduce_add()` is rejected outright; the pin runs both.
  - Upstream's `Float64` is `Scalar[DType.float64]`, a width-one `SIMD`
    with every lane method, and its `Int` converts to `Scalar[DType.int]`.
    Mojito canonicalizes both width-one vectors to the native scalars, which
    `runtime::simd_method` and the checker's SIMD receiver arms skip.
  - A sized scalar (`Float32(1.5)`) at the same binder runs.
  - Found while landing R259 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R411 `Variant` stands a `comptime if` line where upstream calls
  `Self._check[T]()`**

  Problem: upstream's `Variant` methods open with `Self._check[T]()`, whose
  `comptime assert` rejects a `T` that is no alternative; Mojito's
  `stdlib/std/utils/variant.mojo` has a no-op
  `comptime if Self.Ts.contains[T](): pass` there instead.
  - The check itself holds: a storage operation's
    `_get_type_index[T, *Ts]()` fails the instantiation at such a `T` with
    upstream's message (`close_variant_index`, `native::mono`).
  - Port `_InvalidTypeIndex`, `_get_type_index`, and `_check` verbatim,
    now that `comptime assert` runs.
  - Depends on R74.
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

- [ ] **R430 `Int` does not parse a `StringSpan`**

  Problem: `Int(StringSpan("12"))` is `12` at the pin and raises on a
  non-number, while Mojito reports "expected a numeric or Bool value, found
  StringSpan".
  - Workaround: `atol(String(view))`.
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

- [ ] **R432 `StaticString` is an unknown type**

  Problem: a parameter typed `StaticString` compiles at the pin, while
  Mojito reports "unknown type 'StaticString'".
  - Upstream's `StaticString` is `StringSlice[StaticConstantOrigin]`.
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

- [ ] **R850 Float SIMD `//` and `%` are rejected**

  Problem: `Float32(7.5) // Float32(2.0)` fails with "operator '//' is not
  defined for Float32 and Float32", while the pin prints `3.0`, and `%`
  prints `1.5`.
  - Once the VM computes them, add them to `ct::float_lane_binop`, so
    `sccp` folds them too.
  - Found while landing R846 (2026-10-14).
  - Depends on nothing.
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

### Packaging, Artifacts, And Developer Tooling

Track: `tooling`.

- [ ] **R823 The MIR disassembler snapshots are stale**

  `tests/snapshots/mir/{minimal,control_flow,metadata}.mir` still read
  `mojito-mir 1.51` while the writer prints 1.52, so
  `mir_disassembler_test::minimal_program_has_a_stable_snapshot` fails at
  HEAD.
  - The snapshots pin the version header and the declaration rows, so
    every format bump breaks them in a lane the bumping session does not
    run.
  - Refresh them and pin them through `expect_test::expect_file!`, so
    `UPDATE_EXPECT=1` blesses them, as `tests/mir_pass_test.rs` does.
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

- [ ] **R457 HIR types a slot from the first identifier of its name in the
  whole program**

  Problem: `checked_var_types` (`mojito-hir/src/hir.rs`) types each slot of
  a function's CFG from the first checked node anywhere in the program's
  expression arena whose syntax is an identifier of the slot's name.
  - A `comptime for` binder `p` over tuples was typed `Int` from an unrelated
    stdlib `p`; MIR's loop header now types its slot from the binder, but any
    other slot MIR does not retype keeps the stray type.
  - Restricting the scan to the function body's own nodes should change no
    current output, which is why the entry is here.
  - Found while landing R405 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R521 `check_program_for_discovery` and `DiscoveryResult` carry the
  names of the deleted discovery rounds**

  Problem: the checker entry that stops before the checked arena is built,
  and its result type, are named for the discovery rounds R9 deleted.
  - Rename to `check_program_facts` / `CheckedFacts`; `tests/compiler_test.rs`
    and `docs/architecture.md` name both.
  - Depends on nothing.
  - Model: Opus, Planned.

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
