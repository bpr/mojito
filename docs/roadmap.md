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

Next free ID: **R1102**.

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

- [ ] **R1094 A converting default at a parametric struct's implicit
  constructor fails MIR verification since omitted arguments are filled
  at the call**

  `assets/ok/constructor_reads_no_struct_parameter.mojo` stops on the VM
  with "required compile-time value parameter 'n' is missing" in `main`,
  where it ran before.
  - Bisected to f55bbb08 (R1077): `f()` over `def f(t: Tag[3] = 5)` now
    writes `Tag[3](5)` into the call, and the filled construction of an
    owner-less constructor (R1000) does not carry `Tag`'s `n`.
  - Fill a converting default with the constructor's application as the
    parameter's checked type spells it, as the explicit `f(6)` lowers.
  - `benchmarks/instantiation.tsv` keeps the fixture's row from before, so
    the ratchet reports it lost until this lands.
  - Found while landing R1088 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1090 A drop of a tuple's storage names no destructor per element;
  the backends find each element's from the type**

  A `drop.place` or `drop.var` of a pack aggregate (`TyKind::Tuple`,
  `RuntimePack`) names no destructor, since the type has none of its own,
  and the VM's `drop_value` and Pliron's `emit_drop` tear its elements
  down by type (`mir::destroyed_structs`), where upstream's
  `Tuple.__del__` is a `@parameter for` of `destroy_pointee` in which
  destructor insertion names each element's `__del__`.
  - Probe: a `Tuple[String, Int]` dropped (its synthesized destructor's
    `drop.place` of `storage`); a heterogeneous `*args: *Ts` runtime pack
    dropped at the callee.
  - Refine the drop of a pack-typed place into one named `drop.place` per
    element, as `refine_deinit_fields` refines a `deinit` receiver's
    fields, so each element's `__deinit__` is named and respelled as a
    field's is since R1041.
  - An owned `VariadicPack`'s destruction is R1073's.
  - Found while landing R1085 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1079 `inline-parametric` keeps every call that collects or
  spreads arguments**

  A callee with a homogeneous collector (`def total(*xs: Int)`), a spread
  call (`show(*args)`), a keyword collector or forward (`**kwargs`,
  `**kwargs^`), and a `Tuple` member over a computed pack list
  (`Tuple.reverse`, `Tuple.concat`) keep their calls, where the pin inlines
  all four.
  - The VM builds each collector at binding: a pack for `*xs`, a
    `StringDict` for `**kwargs`.
  - The lever: the caller builds the collector, so the collector parameter
    is an ordinary argument R1078's substitution binds.
  - That argument is the pack descriptor R1040 made for `*xs`, the
    dictionary for `**kwargs`, the caller's own collector for a spread, and
    the list expression for a computed pack.
  - The bundled library's `write` wrappers are variadic forwards, so this
    is what lets `print`'s wrappers inline.
  - Probe: `pair.reverse()` and `pair.concat(suffix)` keep their calls
    because their bodies hold a compile-time application over the owner's
    pack (`$comptime$Tuple.reverse$0` over `Ts`), which `inline-parametric`
    refuses to substitute: the elaborator evaluates it against the pack's
    element list, where the substitution would hand it the pack's runtime
    type and it stops with "reads an unresolved type `(Int)`".
  - Probe: `Tuple().reverse()` binds the empty pack and keeps the call for
    the same application.
  - Done when each form inlines in a pass test and `pair.concat(suffix)`
    in `assets/ok/tuple_reverse_concat.mojo` holds no `__getitem_param__`
    call, with a clean differential.
  - Depends on R1078.
  - Model: Fable, Planned.

- [ ] **R1080 `inline-parametric` keeps a call of a nested `def` whose
  closure the caller does not show**

  A nested `def` with a copied or moved capture (`{var x}`, `{x^}`), one
  called through a sibling's captured closure (`def twice() {get}`), one
  declared in a `comptime for`, and a callee passed a capturing closure as
  its callable binder (`apply[plus](3)`) keep their calls, where the pin
  inlines them.
  - Such a body's closure also stays made, stored, and dropped after its
    call inlines, where upstream erases it.
  - A call by name is a direct call passing its captures (R1037); a call
    through a captured sibling closure or of a copy- or move-capturing
    body still goes through the closure.
  - Bind a copied or moved capture to the closure environment's entry, a
    `comptime for` index slot to the caller's loop binder, and a capturing
    callable binder through R1078's map.
  - Erase a closure slot nothing reads but its drop, and its `closure.make`.
  - Done when each shape inlines in a pass test,
    `tests/mir_passes/inline-parametric/nested_captures.mir`'s callers keep
    no closure, and the differential is clean.
  - Depends on R1037, R1078.
  - Model: Opus, Planned.

- [ ] **R1081 `inline-parametric` keeps destructor calls that a drop
  does not spell as a call**

  A `deinit` parameter (`x^.finish()`), a drop inside a region of a slot
  stored before it (`var t = String("x")` dropped in a later `try`), and
  the `drop.place` of a field inside an inlined destructor keep their
  `nodebug` destructors as calls, where the pin inlines all three.
  - Since R1041 a struct with droppable fields has a synthesized `nodebug`
    destructor, whose call at a whole slot's drop inlines into a
    `drop.place` per field naming the field's destructor; the inliner
    binds a `deinit` parameter only to a slot the argument forwards
    (`forwarded_slot`), so a `nodebug` field destructor stays a call at a
    `drop.place`, and an explicit `x^.finish()`, whose receiver is a
    loaded place consumed after the call, keeps its call too.
  - Bind a `deinit` parameter to a fresh slot a non-forwarded argument
    moves into, keeping the callee's consumption of it, and bind one to
    the place an explicit destroy call loads and consumes.
  - Lower a named `drop.place` of a whole field to the call of its
    destructor, as `mir::lower_whole_drops` lowers a `drop.var`.
  - Done when each probe inlines, `S[k]` holding a `D[k]` field drops
    through `D`'s inlined destructor, and the differential is clean.
  - Depends on R1041, R1078.
  - Model: Opus, Planned.

- [ ] **R1096 `conforms_to(T, Trait)` over a concrete binding keeps its
  call inlined**

  `GenericConstraint::Conforms` names a binder, so a `comptime if
  conforms_to(T, Copyable)` in a `nodebug` callee cannot be substituted at
  a concrete `T` and `inline-parametric` keeps the call
  (`tag[Int]()` over `def tag[T: AnyType]() -> Int: return 1 if
  conforms_to(T, Copyable) else 2`).
  - `ParamKind::Conforms { subject, trait_name }` already takes a type
    expression and the elaborator answers it (`specializer.rs`); only the
    constraint form lacks an operand, and `replace_constraint_parameters`
    fails it with "a proposition over `T` at a binding that is no
    parameter".
  - The lever: `Conforms { operand: ConstraintOperand, trait_name }` and
    `ConformsPack` likewise, mapped through `try_map`'s `leaf`, read by
    `availability.rs`'s `constraint_holds` as a type when concrete; the
    checker keeps folding a concrete test to `Bool`, so only the inliner
    emits a type operand.
  - Text format: `conforms { operand, trait }`, the reader accepting the
    1.73 `param:` field as a `Param` operand, and a note in
    `docs/mir-text-format.md`.
  - Done when `tag[Int]()` inlines in a pass test
    (`tests/mir_passes/inline-parametric/conforms_concrete.mir`) and
    `assets/ok/conditional_expression_parameter_condition.mojo`,
    `assets/ok/template_method_generic_witness.mojo`, and
    `assets/ok/bool_alias_predicate_where.mojo` print as before.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1097 `T()` and `T(copy=x)` at a concrete binding keep their call
  inlined**

  `type_param.construct` is rewritten only by the elaborator
  (`mono/substitute.rs`: `default_construct_parameters`,
  `construct_type_parameter`), private to `mojito-native`, so a `nodebug`
  callee constructing its type binder (`make[Int]()` over `return T()`,
  `dup(s)` over `return T(copy=x)`) keeps the call.
  - Both rewrites need only the binder's bound type: the scalar zeros, the
    `MakeSimd` splat, a struct's plain call of the struct, and
    `builtin_copy_is_value_read` or a `copy=` call of the struct.
  - The lever: `mir::construct_type_parameters(blocks, resolve)` in a new
    `crates/mojito-mir/src/mir/type_construction.rs`, the elaborator's two
    functions becoming wrappers that map its error into `MonoError`, and
    `Instantiation::visit_param_ref` leaving a concrete binding in place
    for `instantiated` to run the rewrite.
  - Done when `make[Int]()` prints `0` and `dup(s)`, `dup(7)` print
    `ab 7` with no `make` or `dup` call in `main`, pinned by pass tests
    `construct_type_param.mir` and `copy_type_param.mir`, and
    `assets/ok/always_inline_nodebug_generic_calls.mojo` prints as before.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1098 `canonicalize` keeps a `call.indirect` whose callee is a
  function constant**

  An inlined `apply[double]` body holds `const function(double)` feeding
  a `call.indirect` (`tests/mir_passes/inline-parametric/generic_kept.mir`),
  where upstream's canonicalizer folds the known callee into a direct call
  (MLIR's `SimplifyIndirectCallWithKnownCallee`) and Mojito's elaborator
  does it only per instance, after which no inliner runs.
  - The lever: in `analysis/canonicalize.rs`, a `CallIndirect` whose
    callee register `mir::callable_targets` resolves to a capture-free
    function becomes a `Call` of it, dropping `callee_place`,
    `param_decls`, `instantiated_contract`, and the abstract `resolved`.
  - A closure or a nominal callable struct keeps the indirect call.
  - This is what lets the third round inline `double` when it is itself
    `nodebug`.
  - Done when `generic_kept.mir` shows `call { func: double }`, a sibling
    case `callable_chain.mir` with a `nodebug` `double` shows no call, and
    a `canonicalize` pass case `indirect_known_callee.mir` pins the fold.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1099 `inline-parametric` keeps a call whose argument has no place
  or slot form to bind**

  A `read` parameter binds the caller's place or a fresh slot, and a
  place-bound parameter stands only where the body names it as a place's
  root, a whole-slot copy, a whole-root iteration, or a loan interior
  (`inlined_blocks`), so a body using it otherwise keeps the call, where
  upstream substitutes the argument's address for every use.
  - Probe: `for x in self.items` in a `nodebug` method whose `self` is
    bound through a projection iterates a projected place
    (`iter.init { source }` takes a slot).
  - Probe: a `read` argument read through a reference the block did not
    make (a `ref` parameter's handle passed on) has no place
    (`loaded_place` follows a `ref.make` of the same block only).
  - Probe: `vals[1]` in a `*values: Int` callee keeps
    `ParameterList.__getitem__`, whose returned reference is to a
    temporary span of its own, not to a bound place
    (`returns_bound_place`); on the pin the reference is static.
  - The lever: a slot the caller's block stores the place's value into
    for a borrowed use, as a `read` argument's temporary, and a returned
    reference admitted when its register's origin is static.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R1029 The backends still resolve a call naming a struct to an
  `__init__` overload by arity**

  The concrete program names every compiled constructor a call reaches,
  yet the VM's `Call` arms (`exec.rs`, `dispatch.rs`) and Pliron's
  `constructor_init`/`constructor_init_target` still look one up by arity
  for a call naming a struct, where upstream's backends take the symbol
  the call names.
  - Only a fieldwise struct with no `__init__` and `T(copy=x)` reach
    those arms now.
  - Drop the by-arity lookup there, keeping the VM-synthesized
    constructions (`construct_via_init` with no target) that name no call.
  - Done when neither backend's `Call` lowering resolves a constructor by
    arity and the HEAD differential is clean.
  - Found while landing R1007 (2026-10-15).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1070 A keyword collector's constructor stays as long as its
  struct does**

  `StringDict[T].__init__`, which builds a `**kwargs` collector, stays in
  the concrete program while its struct is live, even once every callee
  declaring that collector is gone, where upstream's call builds the
  collector and names its constructor.
  - The VM's call binding and the native call lowering build the
    collector with no call naming the constructor, so the elaborator
    records it in the struct's member table, which
    `eliminate-dead-symbols` keeps whole.
  - Fix by having MIR lowering emit the collector's construction and one
    `__setitem__` per keyword in the caller, passing it as the forwarded
    `**kwargs^` path already passes one, then drop the table record and
    `mir::CONSTRUCTED_MEMBERS`.
  - Done when a program whose only `**kwargs` callee is dead keeps no
    `StringDict` constructor and the HEAD differential is clean.
  - Found while landing R1030 (2026-10-16), whose slice-bound half landed.
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

- [ ] **R1075 `inline_cost` prices `print` and a `for` loop by
  observation, not by their operations**

  A `print` call costs 28 and an `iter.init` the whole bound in
  `inline_cost`, the figures the pin's probes show, because both are
  builtins here where the pin lowers library code: the `print` call site's
  pack and default arguments, and `range`'s inlined iterator methods.
  - Once each is the library call the pin makes, as `len` becomes through
    R988, the operation count prices it without a figure of its own.
  - Done when both prices are gone and `show`, `show_twice`, and `sum_to`
    in `assets/ok/always_inline_small_calls.mojo` still decide as the pin
    does.
  - Found while landing R923 (2026-10-16).
  - Depends on R988.
  - Model: Opus, Not Planned.

- [ ] **R1076 A comparison of a register with itself is not folded**

  `var s = x` then `if s < x:` tests one value against itself, which the
  pin folds to `False` once `Mem2Reg` has promoted `s`, so a plain
  `@always_inline` body of sixteen such `if`s inlines there and is priced
  as live code here, keeping its call.
  - Fold an integer `lt`, `gt`, or `ne` of one register with itself to
    `False`, and `le`, `ge`, or `eq` to `True`, in
    `MirInstr::folded_constant`; a float comparison stays, for NaN.
  - The slot itself is promoted only once R927 lands, since its arm
    stores to it.
  - Done when such a body inlines, pin-checked, and a `canonicalize`
    pass case pins the fold.
  - Found while landing R923 (2026-10-16).
  - Depends on R927.
  - Model: Opus, Not Planned.

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
    `parameter_ty` and `reference_result`, `TryNext.call`'s types, and the
    `comptime` terminators' parameters.
  - Give `mir::visit` a read-only twin (`TypeVisitor`, `walk_instruction`)
    and run both verifier modes over it, then delete
    `instruction_named_types`.
  - The concrete mode may report findings it does not report today; triage
    them in this entry.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R973 A nested generic `def`'s own unread parameters split its
  instances**

  `gen_inner[m]` nested in `gen_outer` and never reading `m` has one
  instance per `m`, where the pin keeps one.
  - `mir::unread_parameters` (`mir/param_uses.rs`) offers a nested
    function only its enclosing declarations' parameters as candidates,
    never its own.
  - The elaborator already leaves a removed own parameter out of the key
    (`Specializer::own_arguments`); the spelled-application path in
    `specializer.rs` that splices `nested_bindings` before the own
    arguments needs checking with one removed.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R974 A nested `def` in a `comptime for` splits its instances by
  binders it never reads**

  `looped[n]` whose `comptime for` body declares a `def` reading neither
  `n` nor the loop index has four instances of it for `looped[1]` and
  `looped[2]`, and two of `looped`, where the pin keeps one of each.
  - `mir::unread_parameters` gives a function with `region_binders` no
    candidates, so such a body removes nothing and its enclosing function
    reads every parameter through it.
  - The region binders would become candidates too, and
    `Specializer::nested_bindings`, which both `instantiate_nested_bodies`
    and the unroller (`mono/unroll.rs`) call, already drops removed ones.
  - The unroller's generic case names a generator by the arguments, so it
    must agree.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R975 A `def` nested in a parametric struct's method splits its
  instances by the owner's parameters**

  `Counter[1](1).get()` and `Counter[2](2).get()` reach two instances of
  `get` and of its nested `bump`, which read no owner parameter, where the
  pin keeps one of each.
  - A nested body's instance takes `InstanceKey::owner` and the owner's
    method-symbol prefix from the enclosing method's `self_instance`, so
    removing the struct's binders from its arguments still leaves the
    owner in its key.
  - It is the same owner identity R909 drops for a static method, and for
    `get` itself.
  - Depends on R909.
  - Model: Fable, Not Planned.

- [ ] **R1032 A static method's unused argument keeps the parameters its
  type names**

  `S.st[n](a: Arr[n])` returning `3` keeps one instance per `n`, where
  upstream's pass clones the generator without `a` (`_REMOVED_ARG`).
  - `mir::unused_arguments` gives candidates only to a free function and
    a method with a receiver, since a static method is reached through a
    `Call` (`S.st(a)`) and through a resolved `MethodCall` on an instance
    (`s.st(a)`), and each arm must drop the argument.
  - Both arms already call `Specializer::drop_removed_arguments`; the
    static declaration has no receiver slot, so no position shifts.
  - Found while landing R1011 (2026-10-15).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1033 A dunder or table member's unused argument keeps the
  parameters its type names**

  `Grid.__getitem__[n](self, i: Int, a: Arr[n])` or an `__add__` that
  ignores its operand keeps one instance per `n`, where upstream's pass
  clones the generator without the argument.
  - `mir::unused_arguments` gives no candidate to a `__*__` method or a
    `mir::table_member`: an operator, a subscript (`MirSubscriptCall`), an
    iteration step, and the backends' member-table lookups reach them
    without a `MethodCall` arm that drops the argument.
  - Each such demand would take the reduced generator and drop its operand,
    and a member-table entry would name the reduced instance.
  - Found while landing R1011 (2026-10-15).
  - Depends on R1032.
  - Model: Fable, Not Planned.

- [ ] **R1034 A runtime argument a function only passes to itself is a
  use**

  `frec[n](a: Arr[n], k: Int)` returning `frec(a, k - 1) + 1` keeps `a` and
  one instance per `n`, where the pin's `frec…_REMOVED_ARG(i64)` takes
  only `k`.
  - A self-recursive body lies on a cycle, so `Callees::unit_body` gives
    the pass no finished body to read its removed arguments from.
  - Upstream's `RemoveUnusedParams` treats an argument a recursive call
    passes back in its own position as unused.
  - Found while planning R1013 (2026-10-15).
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

- [ ] **R1038 A nested `def` with a copied or moved capture keeps its
  unused arguments**

  `vc(x) {var j}` returning `j + 1` keeps `x`, where the pin's
  `vc(Int)_REMOVED_ARG(i64)` keeps only the capture.
  - A body with a copied or moved capture keeps its statement-made
    closure and is called through it (`call.indirect`), so its calls name
    the full instance, never the reduced generator.
  - Pass the closure's environment entry as the capture argument, as R962
    needs for inlining.
  - Found while landing R1014 (2026-10-16).
  - Depends on R1037, R1080.
  - Model: Opus, Not Planned.

- [ ] **R1039 A nested `def` declared in a `comptime for` keeps its
  unused arguments**

  A nested body with region binders has no candidates in
  `mir::unused_arguments`, where the pin removes its unused arguments.
  - Its calls reach it through `Specializer::region_generators`, which
    names each unrolled copy's generator.
  - A copy's call must name the reduced generator and drop the removed
    operands, as the plain nested path does.
  - Found while landing R1014 (2026-10-16).
  - Depends on R974.
  - Model: Opus, Not Planned.

- [ ] **R1049 A fieldwise construction calls its struct, not a
  synthesized constructor**

  `mkf[n]()` returning `Fw[n](5).v` over `@fieldwise_init struct Fw[n: Int]`
  keeps `n`, where upstream synthesizes `Fw.__init__` and calls it.
  - The struct has no `__init__` to name, so the call stays
    `call { func: Fw }`, an undeclared callee whose types
    `mir::unread_parameters` reads whole.
  - Synthesize the fieldwise `__init__` in MIR lowering, as R1044 did for
    the copy and move constructors, and have the check name it.
  - Found while landing R1020 (2026-10-16).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1050 A copy constructor's declaration does not restate its
  struct's parameters, as an `__init__`'s does**

  `T(copy=x)` calls `T.__copyinit__` with the spelled `T[n]` as its
  `receiver`, where upstream's copy constructor is an `__init__` overload
  whose generator lists the struct's parameters.
  - `lower_method` prepends the struct's `param_decls` only when the
    method is named `__init__`, and the lifecycle rename has already made
    it `__copyinit__` or `__moveinit__`.
  - Restate them for both, then pass a copy construction's struct
    arguments as `instantiated_args`, as any construction does, and drop
    its `receiver`.
  - Found while landing R1020 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1052 An inlined call through a bound still names the trait
  dispatch after its receiver is concrete**

  When `inline-parametric` substitutes `NBox[n]` for a callee's `T:
  MySized`, the inlined `value.__len__()` keeps `resolved:
  "__trait_dispatch.__len__$ov$SelfRead"`, where upstream's substitution
  folds the witness to `NBox.__len__`.
  - `Walker::member_call` (`mir::param_uses`) reads only the member's kept
    struct arguments when the callee's owner is the receiver's struct, so
    the unfolded call reads `NBox[n]` whole and the caller keeps `n`.
  - Fold it the Mojo way: the checker records each declared conformance's
    witness table (requirement dispatch symbol to member symbol) on the
    `StructDeclaration`, and the inliner's substitution respells
    `resolved` through it; never compose `"{struct}.{method}"` (R1027).
  - The same holds for an operator's `resolved`, a subscript call, and an
    iterator step through a bound.
  - The new declaration field bumps the MIR text version.
  - Probe: `fal[n](b: NBox[n])` returning `mylen(b)`, with
    `@always_inline("nodebug") def mylen[T: MySized](value: T)` returning
    `value.__len__()` and `NBox.__len__` `@no_inline`; done when `fal`
    shows `removed_params` and one instance on both backends.
  - Found while planning R1021 (2026-10-16).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R1021 `len(b)` reads its argument's struct whole, whatever
  `__len__` removed**

  `flen[n](b: NBox[n])` returning `len(b)` keeps `n`, where the pin, whose
  `@inline(.always)` library `len` inlines to `b.__len__()`, removes it
  because `__len__` is `_REMOVED_ARG`.
  - Not landable yet: `len(b)` is an undeclared builtin `call`, which
    `mir::unread_parameters` reads whole, and the pin's reduction needs
    the library `len` (R988), plain `@always_inline` inlining (R923;
    a `@no_inline` `len` lookalike keeps `n` on the pin), and the folded
    witness after inlining (R1052).
  - Do not special-case `len` in `param_uses` or lower it as
    `b.__len__()` in the checker: both would freeze the builtin R988
    removes.
  - Once those land, expect no code change: check that `flen` is one
    instance on both backends and add it to
    `assets/ok/member_call_unread_struct_parameter_instances.mojo`,
    pin-checked.
  - Found while landing R998 (2026-10-15).
  - Depends on R1051, R988, R923, R1052.
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

- [ ] **R927 A slot whose reads merge two stores keeps its slot, where
  upstream's `Mem2Reg` carries it through region results**

  `mem-2-reg` promotes only a slot each of whose reads sees one reaching
  store, so a variable assigned in an `if` arm or a loop and read after it
  stays in memory.
  - Upstream's `processRegion` makes such an allocation variant across an
    `hlcf` op and threads it through region arguments, terminator
    operands, and new op results (`Mem2Reg.cpp`, `insertVariants`).
  - MIR is a CFG at the pre-elaboration seam and its blocks take no
    arguments, so the pass solves reaching stores over blocks instead
    (`analysis/mem2reg.rs`).
  - The fix rewrites the pass as `processRegion`'s scoped walk over
    structured bodies with region values.
  - The pass's `comptime for` guard (a read and its store on one side of a
    loop body's boundary) goes once the elaborator clones regions.
  - About two thirds of the promotable-type slot mentions in the
    pre-elaboration bodies of `collections_many` are merged ones.
  - Depends on R879 (structured bodies at the seam), R886 (region values),
    R880 (regions cloned).
  - Model: Fable, Planned.

- [ ] **R928 Aggregate slots are not split, where upstream's `SROA` gives
  each element its own allocation**

  A trivially register-passable struct or tuple slot that is read and
  written through field places stays whole, so `mem-2-reg` never promotes
  it.
  - Upstream's `SROA` runs before each `Mem2Reg`, at both seams, and turns
    a whole-value store into per-element extracts and stores
    (`SROA.cpp`).
  - MIR builds a struct by a constructor call and has no value-level
    construction op for a whole-value read of split elements, so the pass
    needs one, which bumps the MIR text format.
  - Done when a pass test shows a field-accessed `TrivialRegisterPassable`
    struct slot split and promoted, and the corpus effect is recorded.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R926 Generators are not CSE'd before elaboration**

  Upstream's last pre-elaboration round runs `CSEPass` on each generator,
  after `RemoveUnusedParams` and `EliminateDeadSymbols`, and Mojito's
  fourth `PIPELINE` round has no generator pass.
  - It is a missing optimization, not a difference in semantics.
  - R845's pass, registered as a `BodyPass`, is the one to list.
  - Done when `PIPELINE` ends with it and the on/off differential is clean.
  - Depends on R845.
  - Model: Opus, Not Planned.

- [ ] **R929 CSE does not merge reads of memory**

  MLIR's `CSE` merges two identical ops with only read effects when no op
  between them writes memory, and Mojito's `cse` merges only instructions
  free of effects.
  - It is a missing optimization, not a difference in semantics.
  - The unmerged reads are `place.load`, `ref.read`, `var.use` borrows,
    `ref.make`, and a field read through a reference.
  - MLIR checks the ops between the two in one block; a read in a
    dominated block needs no write on any path between, which MLIR
    approximates conservatively.
  - Done when a pass test merges two loads with no write between and keeps
    two with a store between.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R930 CSE leaves values held in storage unmerged**

  `cse` merges only a result held whole in one register, so two identical
  multi-lane SIMD constructions or trivial-struct field reads stay apart,
  where MLIR's `CSE` merges any op free of effects.
  - It is a missing optimization, not a difference in semantics.
  - The guard exists because a reader may consume a register-passable
    value held in storage (an owned argument, an `UninitStorage`
    initialization), as `canonicalize`'s copy forwarding also assumes.
  - Done when a merged storage value's readers are proven non-consuming,
    or a consuming reader copies, and a pass test merges two identical
    `SIMD[DType.float32, 4]` constructions.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R931 `loans.establish` survives into every instance**

  `lower-lit` erases the other lifetime-only markers, but every
  `loans.establish` still reaches the elaborator and both backends, where
  upstream's `LowerLIT` leaves no loan op behind.
  - std's bodies `empty.mojo` reaches hold 253 of them.
  - The VM ignores each one. Pliron reads one only where a `ref` binding's
    sole definition is the marker (`lower_establish_loans`,
    `crates/mojito-pliron/src/lower/vars.rs`).
  - The lever: give that binding a `ref.make` plus `var.store` at MIR
    lowering, then let `lower-lit` erase every `loans.establish`.
  - Done when concrete MIR holds none and `--backend pliron` matches the VM
    on the `ref`-binding fixtures.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R932 Ownership marks outlive the point where upstream erases them**

  `ownership.mark_initialized`, `ownership.mark_destroyed`, and
  `consume.place` reach both backends, where upstream's `LowerLIT` erases
  their LIT counterparts once `CheckLifetimes` has decided every
  destruction.
  - The VM executes all three (`crates/mojito-vm/src/backend/vm/exec.rs`).
  - Pliron clears a presence flag at `ownership.mark_destroyed`
    (`crates/mojito-pliron/src/lower/instr.rs`).
  - The cause is runtime drop flags where upstream's `DestructorInsertion`
    decides statically.
  - Done when drop elaboration leaves no instance that needs a mark, and
    `lower-lit` erases all three.
  - Depends on R931, which settles how `lower-lit` grows past the markers.
  - Model: Fable, Planned.

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

- [ ] **R1092 Natively, a `Dict` or `Set` lookup misses a key it holds
  since small `@always_inline` callees inline**

  `Dict[Int, Int]` with `d[1] = 1` then `print(d[1])` prints `1` on the
  VM and raises `missing key` natively; `"a" in d` answers `False`.
  - Bisected to 68634057 (R923): the native build of 1625dacb runs it.
  - Probe: `assets/ok/dict_methods_growth.mojo`,
    `assets/ok/pliron_dict_set.mojo`, `assets/ok/self_hosted_dict.mojo`,
    `assets/ok/tuple_hashable_dict_key.mojo`,
    `assets/ok/ctfe_module_display_constant_requests.mojo` (`"a" in d`),
    and twelve more `exe-differential` rows that hash a key.
  - The VM runs the same concrete MIR correctly, so some inlined hasher
    or probe body lowers wrongly natively; diff the native result of the
    `AHasher` and `Dict._find_index` instances with the inliner off.
  - `assets/ok/nested_copy_lifecycle.mojo`, R1086's probe, fails the same
    way: check R1086 against it once this lands.
  - Found while landing R1087 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1091 Natively, an owned temporary is rejected when an inlined
  callee's blocks fall between it and its use**

  `(keys[i].copy(), values[i].copy())` fails natively with "unsupported
  owned heap-carrying temporary used across blocks": the second
  subscript's inlined bounds check branches after the first operand's
  `String` copy, which the tuple's constructor consumes blocks later.
  - Bisected to 68634057 (R923), which inlines `List.__getitem__`'s
    small callees; the VM is unaffected.
  - Probe: `Dict.__init__` from key and value lists
    (`assets/ok/collection_display_default.mojo`,
    `assets/ok/container_hashable.mojo`,
    `assets/ownership_ok/shared_iterations_coexist.mojo`, and eleven more
    `exe-differential` rows).
  - Probe: `Codepoint.from_u32(103).or_else(fallback)`
    (`assets/ok/nominal_string_codepoint_from_u32.mojo`) since R1046: the
    forwarded `Optional` temporary's consumer follows the inlined body of
    the copy constructor the `fallback` copy now calls.
  - Pliron's `release.rs` tracks a heap-carrying temporary within one
    block; release it on every edge from its definition to its consumer,
    as an SSA value is live across blocks upstream.
  - Found while landing R1087 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1086 Natively, copying a `Dict` with `Optional` keys loses its
  keys**

  `Dict[Optional[Int], String]` holding `None` and `3`, copied with
  `.copy()`, raises `missing key` at `t2[None]` natively, where the VM
  prints `2 none three three` (`assets/ok/nested_copy_lifecycle.mojo`).
  - `Dict[Int, String]` copies natively; so do a fieldwise struct of
    `UInt64`, `Optional[Int]`, and `String` fields and a `List` of them.
  - Predates R1044: the native build before it fails the same way, and
    the fixture's `exe-differential` row in `conformance/pliron-parity.tsv`
    reports it.
  - Found while landing R1044 (2026-10-16).
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

- [ ] **R591 Natively, forwarding a keyword collector fails to verify**

  Problem: a call forwarding `**opts` into a callee's own `var **opts`
  collector stops natively with "Function call has incorrect type:
  argument 2 type mismatch: expected builtin.integer i64, got llvm.ptr",
  where the VM and the pin run it.
  - `assets/ok/trait_default_wrapper_conventions.mojo` hits it through a
    trait default's wrapper, so its `exe-differential` parity row is
    stale; HEAD before R551 fails the same way.
  - The native call lowering builds a fresh collector from the keyword
    overflow, so the forwarded `**` collector lands as one element.
  - Found while landing R551 (2026-10-10).
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

- [ ] **R277 Native `Tuple.concat` double-frees a `String` element of its
  argument**

  Problem: `var c = u^.concat((String("x"), 3.5))` prints the result and
  then traps with "vm: double free of Pointer allocation" natively, where the
  pin and the VM run to completion.
  - `reverse`, and `concat` over a `String` in the receiver, run natively
    since the members move their elements out through pointers
    (2026-10-06).
  - An argument of `Int`s is clean, and so is one whose element is a struct
    with a destructor (`assets/ok/named_result_transferred.mojo`).
  - The member takes each element of `other` through a pointer and marks
    `other.storage` destroyed, so the suspect is the presence flag of a
    `deinit` parameter that is not the receiver.
  - Pinned by `conformance/probes/native_tuple_transform_string_element.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R469 Native `Tuple` construction copies a fresh struct element in
  and leaks the original**

  Problem: `var t = (1, Q(8, "x"))` runs `Q`'s copy initializer natively and
  never destroys the temporary, where the pin and the VM move it in and
  print `1 / del 8 / end`.
  - Native prints `copy 8 / 1 / del 8 / end`.
  - Every read of a value parameter holding such a struct constructs one
    (R466), so the copy shows once per read there.
  - Pinned by `conformance/probes/native_tuple_struct_element_copied.mojo`.
  - Found while landing R466 (2026-10-07).
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

- [ ] **R832 Native drops are flag-guarded, and every return sweeps the
  flagged slots**

  Problem: native lowering guards every `drop.var` with a dynamic drop
  flag and drops every still-flagged slot at each return, where upstream
  inserts static destructors with no flags.
  - `lower_drop_var` (`lower/drops.rs`) checks the flag, and
    `lower_return` (`lower/term.rs`) sweeps the flagged slots.
  - The MIR verifier holds every drop and every cleanup list to a slot
    some path still holds, so a slot's state is static wherever no two
    paths disagree.
  - Fix: drop the flag where the slot's state is static, and file what
    remains (the sweep's comment names compiler-private `StringDict`
    shells).
  - The sweep is the one drop no MIR instruction names: it finds each
    slot's destructor by type (`FnLowering::type_destructor`), where every
    `drop.var` and cleanup-list entry names its own since R1016 and R1042.
    A slot the sweep still has to drop is one drop elaboration left no
    `drop.var` for; once each has one, the lookup goes with the sweep.
  - Found while landing R826 (2026-10-14).
  - Depends on R831 (one list over several raise points needs the flag).
  - Model: Fable, Planned.

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

- [ ] **R473 A discard `_ = x` copies its source instead of taking it**

  Problem: `var x = Q(1, "y")` then `_ = x` as `x`'s last use prints
  `del 1` twice in Mojito, where the pin destroys one `Q`.
  - The same holds for `var r = q; _ = r` over a value parameter `q`.
  - Mojito copies `x` into the discard and destroys both, where the pin
    moves the last use.
  - Found while landing R467 (2026-10-07).
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

- [ ] **R1064 Constructing a `Variant` copies the value it takes**

  Problem: `Variant[CBox[n], Int](b.copy())`, over a `CBox` whose copy
  constructor adds 100, reads back `201`, where the pin reads `101`.
  - The pin's `Variant.__init__` takes `var value: T` and moves it in;
    the VM's `variant.make` (`copy_named`) and Pliron's
    (`copy_named_aggregate` through `copy_reg_into` in `variants.rs`) copy
    the payload again, by the constructor the store names since R1045.
  - Make `variant.make` take its operand, after checking MIR lowering
    copies a borrowed argument first; `variant.set` and
    `variant.replace` may share the fix.
  - Found while landing R1010 (2026-10-16).
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

- [ ] **R733 An outer value last used after a raise in a `try` body is
  destroyed after the handler on the raise path**

  Problem: `var m = M(2)` then `try: _ = may(-1); var d = M(copy=m)`
  prints `caught2`, then `mdel 2`, where the pin destroys `m` on the raise
  edge and prints `mdel 2` first.
  - The raise happens before `m`'s last use, so the pin treats `m` as
    dead from the raise edge on.
  - Values declared inside the body already die on the raise edge.
  - Found while landing R723 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R833 A raising operator destroys its left operand after values
  declared later**

  Problem: in `assets/ok/try_raise_edge_drop_order.mojo`, `var x = a +
  S(9)` raising in a body that declared `a`, then `b`, prints `del 9`,
  `del 2`, `del 1`, where the pin prints `del 9`, `del 1`, `del 2`.
  - `a`'s last use is the raising `+`, and the pin destroys it on that
    raise edge before the slots still live past it.
  - Mojito's raise-edge cleanup list drops every held slot in reverse
    declaration order.
  - Found while landing R826 (2026-10-14).
  - Depends on R831 (a raise edge per raising instruction orders its own
    drops).
  - Model: Opus, Not Planned.

- [ ] **R829 A value a `return` holds past an inner `finally` is destroyed
  after the outermost one**

  Problem: a return crossing `try: try: return i finally: print("inner",
  s.tag)` inside `try: ... finally: print("outer")` prints `outer` before
  `drop 2`, where the pin prints `inner`, `drop 2`, `outer`.
  - A return's cleanup list runs once, after every `finally` it crosses.
  - The pin destroys the value on the edge after the last `finally` that
    reads it.
  - Field drops (`field_drops.rs`) still seed a region's returns with the
    `finally`'s normal-path liveness.
  - Found while landing R826 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Planned.

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
  - Found while landing R826 (2026-10-14).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R830 An escape's cleanup list runs before the `finally` it
  crosses**

  Problem: a `break` out of a `try` whose `finally` reads `s`, with `t`
  read only in the body, prints `breaking`, `drop 3`, `finally 2`, where
  the pin destroys `t` before `breaking`.
  - Escape lists are filled from the `try`'s entry liveness
    (`fill_escape_cleanups`), not from the liveness at the escape.
  - A value the `finally` reads and the target does not survives the
    escape, and dies only where the target block happens to drop it.
  - Fix: an escape-channel seed like the return channel's `return_live`,
    with the list run after the crossed `finally` blocks.
  - Found while landing R826 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R891 A `break` out of a `try`/`finally` in `for x in xs` over a
  named `List` destroys `xs` before the `finally` reads `x`**

  `for x in xs: try: … break … finally: print(x)` fails in the VM with
  `ReadRef … invalid reference projection Field("data") on None`.
  - The escape's cleanup list holds the iterator and `xs` and runs before
    the `finally`, which reads `x`, a reference into `xs`.
  - `continue` across the same `finally`, and `break` with an `except`
    and no `finally`, both run correctly.
  - It is very likely R830's case: run the list after the crossed
    `finally`, then add this program as a fixture.
  - Found while landing R872 (2026-10-14); it fails on `aae331e8` too.
  - Depends on R830.
  - Model: Opus, Not Planned.

- [ ] **R831 A `try`'s raise-edge cleanup list serves every raise point of
  its body**

  Problem: a slot ended before one raising call but held at another stays
  listed, and the VM's empty-slot no-op or the native drop flag absorbs its
  teardown when the first call raises.
  - Upstream destroys on each raise edge what that edge holds.
  - The MIR verifier accepts the list, since some raise path holds each
    entry.
  - Lever: an explicit raise edge per raising instruction in a `try` body.
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
    owned operand gets no `$tmp_arg_r` slot.
  - Found while landing R713 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R716 A call temporary read inside a t-string dies before the
  print**

  Problem: `print(t"{S(3).v}")` prints `del 3` before `3`, where the pin
  prints `3` and then `del 3`.
  - The temporary should live until the enclosing `print` returns.
  - Found while landing R713 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R688 An owner whose last use is a `var` aggregate argument's copy
  lives through the call**

  Problem: `obj.own2(y=h1.c, x=h2.c)` over `def own2(self, var x: C,
  var y: C)`, with a printing copy initializer and destructor, prints
  `copy 3, del 3, copy 2, del 2, own2 3 2` at the pin.
  - Mojito copies in the same order but destroys both owners after the
    call: `copy 3, copy 2, own2 3 2, …, del 3, del 2`.
  - The copied register keeps its owner's provenance, so drop elaboration
    retains the owner through the call. The fix is the `CopyValue` result
    provenance in `crates/mojito-analysis/src/analysis/register_loans.rs`.
  - Found while landing R681 (2026-10-13).
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

- [ ] **R352 A pointer whose origin was cast reads the origin's place, not
  its pointee**

  Problem: `var p = Pointer(to=b).unsafe_origin_cast[origin_of(a)]()` then
  `print(p[])` prints `a`'s value in Mojito, where the pin prints `b`'s.
  - `pointer_deref_place` (`crates/mojito-mir/src/mir.rs`) substitutes the
    loan's place for the pointer's target.
  - Depends on nothing.
  - Model: Opus, Planned.

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

- [ ] **R496 A reference returned through a temporary `Span` is stale on
  the VM**

  Problem: `def first(ref l: List[Int]) -> ref[l] Int: return Span(l)[0]`
  stops on the VM with "stale reference to frame N", where the pin prints
  the element.
  - A VM reference handle is a frame slot plus a projection, so an element
    reached through the temporary span's pointer stays rooted at the slot
    the body drops before it returns.
  - A result bound to a `ref[ImmStaticOrigin]` register is already
    forwarded as the element's heap pointer
    (`VmBackend::static_reference_result`), which serves
    `ParameterList.__getitem__`.
  - The general fix is a reference handle that designates heap storage
    whenever its projection crosses a pointer.
  - Since R1078 a `ref.make` into a static-origin register is forwarded
    the same way, so an inlined `Span.__getitem__` over a temporary span
    survives too, and a place projected from a pointer slot (`q[i]._size`,
    a span member inlined at a pointee place) reads and writes the pointee
    (`references::pointer_element`).
  - Probe: a reference to a part of a heap element that is not itself a
    pointer's element (`q[i].field` taken by `ref`, as `Optional.value`
    inlined at a pointee place makes it) stops with "a reference within a
    heap element", since a handle rooted in the heap has no form.
  - Found while landing R325 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R535 A `ref` field of a list element prints its handle inside `print`**

  Problem: over `l = [RefOne(a)]`, where `RefOne` holds `var value: ref[o] Int`, `print(l[0].value)` prints `<ref 1:0>` instead of `4`.
  - The same happens for a `ref` field of a `Tuple` field's element.
  - `var z = l[0].value` binds, but `print(z)` then prints `None`.
  - Inside the kept direct-`ref`-field extension, so its fixture goes under `assets/extensions/ok/`.
  - Found while planning R529 (2026-10-09).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R956 A raising initializer's initialized fields are not destroyed
  when it raises**

  Problem: `self.a = Tracer("a")` then `raise` in a raising `__init__`
  leaks `a`, where the pin destroys each field initialized before the
  raise.
  - Inlined into a `try`'s body (`inline-parametric`), the `out self` slot
    stays off the raise-edge cleanup, since a slot list cannot end part of
    a value.
  - Kept as a call, the raise leaves the initializer holding the fields,
    which R20 covers for every function.
  - Fix: end the fields the raise holds, one `DropPlace` each, on the
    raise edge R20 settles.
  - Probe: a `Pair.__init__` setting `a`, raising, then setting `b`,
    called in a `try` (found while landing R919, 2026-10-14).
  - Depends on R20.
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

- [ ] **R536 Copying out of a `ref` binding keeps the binding's loan**

  Problem: `ref a = left; var t: Int = a; left = 9; print(t, left)` is rejected with "access to 'left' conflicts with live reference 't'"; the pin prints `4 9`.
  - The same holds for `var t = a`, `(a, 5)`, `[a, 5]`, and an `Int`-returning call `keep(a, 5)`.
  - The copy is an owned `Int`, so it carries no loan on `left`.
  - Found while planning R529 (2026-10-09).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R1082 A read of a local is rejected while a `ref` binding to it is
  live**

  `ref r = kk; var a = kk + 1; return a + r` is rejected with "access to
  'kk' conflicts with live reference 'r'"; the pin runs it.
  - The `ref` binding's loan is exclusive, so any read of its referent
    before the binding's last use conflicts with it.
  - Probe: the same read through a call (`reads(kk)`) and through a
    nested `def cap(x) {imm kk}` called by name beside `r`, whose `imm`
    capture is a read parameter of the lifted body; the pin runs all
    three (`conformance/probes/read_beside_live_ref_binding.mojo`).
  - Found while landing R1037 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R742 An indexed element of a `var` collector cannot be mutated**

  Problem: `xs[0].v += 7` over `def refs(var *xs: M)` is rejected with
  `type '$variadic[M]' cannot be indexed here`.
  - Upstream's owned `VariadicListMem` hands out mutable element
    references, so the pin is expected to run it; observe the pin first,
    since its environment was missing when this was filed.
  - `for ref x in xs: x.v += 10` over the same collector already runs.
  - The rejection comes from the checker's pack-index place rule
    (`crates/mojito-checker/src/checker/indexing.rs`).
  - Found while landing R734 (2026-10-13).
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

- [ ] **R354 A `deinit self` method reading a `Tuple` field's element beside
  a second `deinit` parameter traps in the VM**

  Problem: `def swap(deinit self, deinit other: Box2) -> Int: return
  self.t[0].id + other.t[1].id`, `t` a `Tuple[Token, Token]` of non-copyable
  elements, stops with "invalid reference projection Field("storage") on
  <moved>", where the pin prints `5`.
  - Each receiver's field is destroyed at its last use, and the element
    read reaches the storage after that.
  - Depends on nothing.
  - Model: Opus, Planned.

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
  - The rejection is the checker's implicit-copy test on a reference
    result (`crates/mojito-checker/src/checker.rs`), which treats the
    `ref` slot as a consuming read.
  - Found while landing R700 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R724 `S(copy=s)` over a synthesized copy constructor fails at run
  time**

  Problem: `var d = S(copy=s)` over a `@fieldwise_init struct S(Copyable,
  Movable)` with no spelled copy constructor stops with "unsupported
  feature: vm: struct 'S' has no copy constructor", where the pin prints
  the copy.
  - A spelled `__init__(out self, *, copy: Self)` runs.
  - The explicit `copy=` construction does not reach the synthesized
    copy the implicit copy path uses.
  - Found while landing R717 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R679 Returning a field of a call's reference result is rejected**

  Problem: `def outer(ref w: W) -> ref[w.t] Int: return inner(w).t`, over
  `def inner(ref w: W) -> ref[w] W`, fails with "reference binding to a
  non-place expression", where the pin prints the field.
  - The same holds over read parameters with `origin_of(w.t)`.
  - Spelled `-> ref[origin_of(w)] Int`, the pin rejects it with "cannot
    return reference with incompatible origin", and Mojito keeps its own
    words.
  - Found while landing R672 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R677 A read argument reaches the VM callee as a frame value unless
  the result borrows it**

  Problem: upstream passes every memory-only `read` argument by address,
  but the VM binds a read argument as the caller's storage only when the
  call's result borrows it.
  - A borrowing view or a reference result lends its read arguments
    (`lends_read_arguments`); every other call copies the value into the
    callee frame.
  - Nothing observes the difference yet: pointer comparison on an
    origin-bearing pointer is rejected, so `UnsafePointer(to=w)` inside the
    callee cannot be compared with the caller's.
  - It becomes a defect once pointer identity lands.
  - Found while landing R672 (2026-10-13).
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

- [ ] **R827 Field drops and copy elision take every call as raising**

  `field_drops.rs` and `copy_elision.rs` treat every instruction outside a
  small silent list as a raise point (`scan::may_raise`), where drop
  elaboration now seeds a handler only at checked raise points
  (`scan::raises_to_handler`).
  - A non-raising call can keep a field live past its move, the field-level
    form of the dead drop R825 fixed for whole slots.
  - The R825 verifier rule checks whole slots only, so it does not catch it.
  - Fix: switch both to `raises_to_handler`, and compare `mojito run`
    output on the `try` and `__deinit__` fixtures against the old binary.
  - Found while landing R825 (2026-10-13).
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

### Catch Up To Current Mojo: Calls, Overloads, Conversions, And Traits

Track: `calls`.

Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

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

- [ ] **R719 A raising prefix dunder is not a raising call**

  Problem: `var x = -S(3)` over `def __neg__(self) raises -> S` in a
  non-raising `main` runs, where the pin rejects it: "cannot call function
  that may raise in a context that cannot raise".
  - `infer_prefix` (`checker/operators.rs`) takes no span, so it records
    no call effect and requires no raising context, as `infer_infix` now
    does for a struct infix dunder.
  - MIR therefore cannot tell a raising prefix dunder from a non-raising
    one, and `produced_in_place` never binds a prefix result in place.
  - Found while landing R714 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

  Problem: `def g[T: Iterator](mut it: T)` doing `_ = it.__next__()`
  checks, but the pin rejects it: "'(expression temporary)' abandoned
  without being explicitly destroyed".
  - `Iterator.Element` is bound only by `Movable`. Upstream's own
    `_DropIterator` asserts `conforms_to(Self.Element, Deinitable)` before
    it discards one.
  - The cause: `is_deinitable` and `is_movable`
    (`crates/mojito-checker/src/checker/traits.rs`) fall through to
    `leaf_conforms(..).unwrap_or(true)` for every `Assoc`.
  - The lever: answer an `Assoc` from `projection_bounds`, as
    `assoc_member_bound_proves` already does for `Copyable`.
  - Expect fallout in generic bundled-std bodies that drop a projected
    value.
  - `moving_an_iterable_while_its_generic_iterator_lives_rejects`
    (`tests/vm_test.rs`) discards an element. If it starts failing for
    this reason, give it a non-discarding read.
  - Found while landing R665 (2026-10-13).
  - Depends on nothing.
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

- [ ] **R510 A user trait named like a prelude trait replaces it**

  Problem: a module declaring `trait Sized: comptime size: Int` prints `1`
  at the pin and stops in Mojito with "struct 'Tuple' declares conformance
  to trait 'Sized' but is missing comptime member 'size'".
  - The user declaration takes the name the bundled library's conformances
    resolve to, where at the pin it shadows the prelude trait in its own
    module alone.
  - Probe: `conformance/probes/user_trait_named_like_prelude.mojo`.
  - Found while planning R488 (2026-10-08).
  - Depends on nothing.
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

- [ ] **R943 A documentation decorator on a trait method is rejected**

  Problem: `@doc_hidden` or `@stable(since="1.0")` on a trait method is
  accepted at the pin, but Mojito rejects it as an unsupported
  trait-method decorator.
  - The checker accepts only inline-level decorators on a trait method
    (`check_trait`), so an unknown one stays an error as at the pin.
  - The fix accepts the pin's documentation decorators by name and keeps
    rejecting the rest.
  - Found while landing R914 (2026-10-14).
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
    `_ParameterListIter[*Self.values](0)` where upstream writes `{0}`.
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

- [ ] **R409 A template-served method's violated `where` clause is spelled
  from its folded constraint**

  Problem: `v.set[Conn](Conn(4))` on a `Variant[Conn]` whose `Conn` is not
  `Deinitable` reports "expected 'conforms_to(Ts.values, Deinitable)'",
  where the clause is declared, and the pin quotes it, as
  `Ts.all_conforms_to[Deinitable]()`.
  - Both spellings fold to one `GenericConstraint::ConformsPack`, and
    `violated_constraint_reason` (`checker/generics.rs`) prints its
    `Display`.
  - A method the cloner specialized was unparsed from its source
    (`comptime/unparse.rs`), as `Variant`'s were until 2026-10-06.
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
  - Pinned by `conformance/probes/keyword_through_bound_renamed_witness.mojo`
    (the pin prints `312`).
  - The rejection is `Checker::record_bound_default_arguments`
    (`checker/bound_defaults.rs`). It also covers a requirement default the
    check would spell by a renamed name.
  - Cause: a call through the bound reaches the witness by name, in a clone
    re-checked nominally and in `native::mono`'s `infer_call` alike.
  - The lever is to lower such a call positionally, by the requirement's
    binding, which the checked call contract already records.
  - Found while landing R418 with R414 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R548 A homogeneous variadic cannot be forwarded with `*xs`**

  Problem: `return g(*xs^)` in `def f(var *xs: Int)`, where `g` takes
  `var *xs: Int`, runs at the pin, while Mojito reports "call spread
  outside a specialized type pack".
  - Only a type pack (`*args: *Ts`) is forwarded today
    (`lower_pack_spread_argument` in `mir/calls.rs`).
  - So a trait default with a homogeneous collector (`def f(self, var *xs:
    Int)`) is rejected at each conformer that inherits it, since its
    forwarding wrapper spells that spread (`checker/trait_defaults.rs`).
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

- [ ] **R437 `o = String(x)` into an `Optional[String]` fails MIR
  verification**

  Problem: `var o = Optional[String](None); o = String(3)` prints `3` at the
  pin, while Mojito stops with "argument 0 of
  'Optional$mono$TString.__init__$ov$T$AnyType' has type StringLiteral,
  declared String".
  - The checker records the `String` wrap of the builtin conversion and the
    `Optional` conversion at the same span, and `implicit_conversions` keeps
    one of them.
  - Workaround: spell `Optional[String](String(3))`.
  - Found while landing R421 (2026-10-06).
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

### Catch Up To Current Mojo: Compile-Time Parameters, Packs, And Reflection

Track: `comptime`.

Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

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

- [ ] **R979 `Self.x` of a struct parameter typed `Float64` reads as
  `Int`**

  `struct F[x: Float64]` whose static `get() -> Float64` returns `Self.x`
  is rejected with "type mismatch for return: expected Float64, found
  Int", where the pin prints `1.5`.
  - `infer_member` (`crates/mojito-checker/src/checker/indexing.rs`) reads
    a value parameter at its declared type only for `UInt`, `Bool`, SIMD,
    struct, and `DType` types, and every other one as `Int`.
  - A read at the declared type is a parameter constant; the rest still
    lower to a projection of a `self` slot, which a static method interns
    for it (`intern_static_self`) and which reads every struct parameter.
  - Reading every type as declared removes both.
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

  Problem: `comptime Twice[n: Int] = f(n)` applied as `Twice[3]` reports
  "a compile-time call of 'f' is not evaluated above the check", where the
  pin evaluates the alias body in its elaborator and prints `6`.
  - The AST route that evaluated the body is gone (R9); the alias body is
    folded by `Elab::apply_generic_alias` above the check, which folds no
    call.
  - The lever is the one the module constants took: the check binds the
    application as a parameter expression and `native::mono` evaluates it.
  - A struct's associated `comptime` member that applies a callable
    (`comptime k = f(1)` inside a struct) was already refused before R9
    ("not an associated comptime expression") and takes the same lever.
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
    `select_comptime_branches`, `mono/specializer.rs`), so the header's
    thunk runs in an arm the instance never takes.
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

- [ ] **R395 A struct's `Bool` parameter read as a value is typed `Int`**

  Problem: `def on(self) -> Bool: return Self.b` in `struct Flag[b: Bool]`
  is rejected with "type mismatch for return: expected Bool, found Int",
  where the pin prints `True`.
  - `self.x = 1 if Self.b else 0` passes the check and runs on the VM. The
    native backend rejects it with "Op has different operand types".
  - A `Bool` parameter of a `def` read the same way is a `Bool`.
  - Found while landing a function applied in a signature (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R848 A struct's `Bool` or `Float64` value parameter read as
  `Self.b` is typed `Int`**

  `struct Flag[b: Bool]` with `def get(self) -> Bool: return Self.b` is
  rejected with "type mismatch for return: expected Bool, found Int", where
  the pin prints `True`; `Scale[f: Float64]` with `Self.f * 2` fails the same
  way.
  - The checker's `infer_member` (`checker/indexing.rs`) reads a struct
    value parameter at its declared type only for `UInt`, SIMD, struct, and
    `DType` parameters, and at `Int` otherwise.
  - R790 is the same fallback for a `String` parameter, so one fix may close
    both.
  - Found while landing R841 (2026-10-14).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R790 A struct's `String` value parameter read as `Self.s` is typed `Int`**

  `struct Box[s: String]` with `def get(self) -> String: return Self.s` is rejected with "type mismatch for return: expected String, found Int", where the pin prints the string.
  - A free function's `[s: String]` and `[q: Q]` reads (`q.s` a `String` field) both work; the struct member read is the one that mistypes.
  - `native::mono` already answers such a read with a `String`-typed constant (`Specializer::value_param_constant`), so the fix is in the checker's member read.
  - Found while landing R524 (2026-10-13).
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
  - Found while planning R7 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R801 A `comptime` value member or alias cannot take parameters**

  Problem: `comptime get[idx: Int]: Int = Self.values[idx]` on a struct,
  and `comptime plus_one[n: Int]: Int = n + 1` at module scope, fail with
  "a compile-time value member cannot take parameters", where the pin
  declares upstream's `ParameterList.__getitem_param__` this way.
  - Only types are templated today (`ParameterizedMember.template: Ty`,
    `lower_parameterized_member` in `checker/constraints.rs`,
    `StructDeclaration::parameterized_associated_types`).
  - An unannotated member fails with "a parameterized associated type must
    be defined by a type".
  - A module alias read with a symbolic argument (`plus_one[k]` in a
    generic `def`) fails with "Undefined variable".
  - The member's template must cross the package boundary in the struct
    record, so the change bumps the package format and the MIR schema.
  - Found while planning R494 (2026-10-13).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R806 A parameterized alias read through a struct is not parsed**

  Problem: `S.Of[Bool]`, `S[Int].Of[Bool]`, and `ParameterList.of[4, 5, 6]`
  are rejected, where the pin reads a struct's parameterized `comptime`
  alias this way.
  - The expression parser stops at `call_name` (`parser.rs`): "only named
    functions can be called, found Member".
  - In type position, `var a: S[Int].Of[Bool]` fails with "a dependent type
    projection requires exactly one compile-time value index".
  - With call parentheses, `S[Int].Of[Bool]()` fails in the checker with
    "a parameterized type is not a value".
  - Update `docs/grammar.md` first.
  - It blocks `ParameterList.of` (R809).
  - Found while planning R493 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R807 `ParameterList` takes a variadic `*values: type`, not
  upstream's one raw-list parameter**

  Problem: upstream declares `ParameterList[type, //, values:
  _MLIR.KGENParamListType[type]]`, so `vs.values` and
  `ParameterList[vs.values]` have no spelling in Mojito.
  - Port upstream's `struct _MLIR` with `KGENParamListType[elt_type]` as a
    compiler-known type, the type of today's `MetaTy::ParamList`.
  - Make `vs.values` project a pack's raw list, and respell the checker's
    runtime read (`Checker::infer_value_pack_read`) as
    `ParameterList[vs.values]()`.
  - Admit a user's `ParameterList[vs.values]`, and delete
    `admit_parameter_list_brackets` and its helpers.
  - Port `__iter__`'s `_ParameterListIter[*ParameterList[...]()]` spread.
  - Likely bumps the MIR schema and the package format.
  - A session should plan this first and split it if it does not fit.
  - Found while planning R493 (2026-10-13).
  - Depends on nothing.
  - Model: Fable, Planned.

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
    (`Checker::reads_raw_parameter_list`).
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

- [ ] **R1015 A runtime default that prints is rejected**

  `fd2(x: Int = side())` returning `x`, where `side` prints, fails
  elaboration with "is not safe for VM-backed compile-time execution: it
  reaches `print`", where the pin prints `side` when `fd2()` runs.
  - The default is lifted to `$default$fd2$x`, which the elaborator runs
    at compile time (`effectful_callee`, `mono/specializer.rs`), while
    upstream evaluates a runtime default at the call site when the call
    runs.
  - A call omitting a removed argument's default no longer evaluates it
    (R997), so only a used argument fails.
  - Found while landing R997 (2026-10-15).
  - Depends on R1004.
  - Model: Opus, Not Planned.

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
  - Found while landing R446 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R449 A keyword-supplied infer-only parameter is rejected**

  Problem: `f[T=Int, n=3](1)` over `def f[T: Writable & ImplicitlyCopyable,
  //, n: Int](x: T) -> Int`, and `PL[T=Int]()` over `struct PL[T: …, //,
  *vs: T]`, fail with "infer-only parameter 'T' cannot be supplied
  explicitly", where the pin accepts both and prints `3` and `0`.
  - The pin rejects only a positional argument for an infer-only binder.
  - The check is made twice: in `Checker::resolve_use_params` and in
    `split_callable_specialization` (`checker/generics.rs`).
  - Found while landing R446 (2026-10-07).
  - Depends on nothing.
  - Model: Fable, Not Planned.

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
  - Found while landing R494 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R804 The length of a value pack in a signature type is rejected**

  Problem: `def sized[*vs: Int]() -> Box[len(vs)]` fails with "not a
  compile-time Int constant: not an associated comptime expression", where
  the pin types the result by the pack's length.
  - A signature has no body to lift `len(vs)` from, and `len` is a checker
    builtin rather than a module function, so `called_application` does not
    serve it.
  - `Box[vs.size]` is the spelling that works today.
  - Found while landing R494 (2026-10-13).
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
    from (R449 covers the keyword form).
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

  Problem: in `struct T[p: Tuple[Int, Int]]`, `var t = Self.p` fails with
  "no field 'p'" and `Self.p[1]` with "Undefined variable 'Self'", where
  the pin prints both elements of `T[(1, 2)]`.
  - A `Tuple[Int, String]` parameter fails the same way.
  - A struct's scalar or struct-typed value parameter reads (`Self.n`,
    `Self.e.rows`).
  - Found while landing R455 (2026-10-07).
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

- [ ] **R500 A trait's compile-time value member read through a type
  parameter in a compile-time position is rejected**

  Problem: `comptime if T.K > 2:` or `SIMD[DType.int32, T.K - 1]` in `def
  g[T: HasK]()`, where `HasK` declares `comptime K: Int`, fails with "not a
  compile-time Int constant: unsupported associated comptime member
  access", where the pin folds `K` per instance.
  - `comptime for i in range(T.K)` in a method `m[T: HasK]` fails where a
    call reaches it ("function instantiation of `S.m` failed"). It ran
    before R310, on a per-call clone that folded `T`; a generic `def` with
    the same loop still clones and runs.
  - No parameter expression names an associated value of a type binder,
    and `native::mono` has no table of struct associated values to answer
    one.
  - Probe: `conformance/probes/trait_value_member_comptime_position.mojo`.
  - Found while landing R310 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R505 A trait's compile-time value requirement cannot declare a
  default**

  Problem: `comptime K: Int = 3` in a trait body fails to parse ("Expected
  newline, ';', or EOF at the end of statement"), where the pin accepts it
  and a conformer that omits `K` reads `3` through a bound (`T.K`).
  - The parser's trait `comptime` member takes no initializer; a struct's
    annotated member (`comptime N: Int = 3`) parses.
  - Conformance would then take the default for a witness that omits the
    member, and `ConformanceFacts::associated_values` would carry it so the
    elaborator answers `T.K` from it.
  - Found while landing R484 (2026-10-08).
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
  - Tuple targets (`comptime for a, b in …`) are unverified too.
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
  - The checker accepts a spread only of a type pack
    (`checker/inference.rs`).
  - Upstream forwards the caller's `VariadicList` whole; since R1040 the
    pack is one run-time value on both backends, so the forward can pass the
    caller's pack, natively its descriptor, unchanged.
  - Found while landing R1040 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R436 A struct's `comptime` value member read through the type name
  is rejected**

  Problem: `print(E.k)` over `struct E: comptime k = 3` prints `3` at the
  pin, while Mojito reports "Undefined variable 'E'", and `G(E.k)` reports
  that no constructor overload matches.
  - R279 is the same member read as `Self.k` inside the struct.
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
  - Library types leak the same way (`__module$list$List`, and R68's
    `__module$hasher$default_hasher`).
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
  declares, such as `Boolable` or `Sized`**

  Problem: `comptime if conforms_to(T, Sized)` holds for a plain
  `@fieldwise_init` struct, `Optional[Int]`, and `String` in Mojito, where
  the pin answers no for all three.
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
    `Sized`.
  - Unlike R577's traits, every fact is already at hand: a struct's
    `declared` rows, and `types::trivially_register_passable_leaf` for the
    compiler-known types upstream declares it on.
    `MirDeclarations::copy_is_identity` already answers this way below the
    waist.
  - Pinned by
    `conformance/probes/conforms_to_trivial_register_passable_undeclared.mojo`
    and `conformance/probes/trivial_register_passable_bound_undeclared.mojo`.
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
  - Likely the same missed refinement as R861; R724 is the `copy=`
    construction half for a declared `Copyable`.
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

- [ ] **R534 A closure with a by-value capture is accepted as a
  compile-time callable argument; the pin rejects it**

  Problem: `invoke[inner]()`, where `inner` is `def inner() {var x} -> Int`
  and the parameter is `callback: def() capturing[origins] -> Int`, prints
  10 in Mojito; the pin rejects it ("cannot use a dynamic value in a
  parameter list").
  - A generic closure of the same shape is refused, but only by the
    elaborator ("captures by value"), not by the checker at the binding.
  - Following the pin means the checker rejecting a by-value capturing
    closure as a parameter argument.
  - Pinned by `conformance/probes/callable_parameter_by_value_capture.mojo`.
  - Ledger name: `callable-parameter-by-value-capture`.
  - Found while landing R526 (2026-10-09).
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

- [ ] **R816 A contiguous `List` slice is a copy, not a borrowing `Span`**

  Problem: `var ys = xs[1:3]; xs.clear(); print(len(ys))` prints 2 in
  Mojito, where the pin reports "use of invalidated interior reference
  'xs["element"]'".
  - The bundled `List.__getitem__(ContiguousSlice)` returns `Self`, where
    upstream's returns a `Span` over the list's elements.
  - Following the pin changes every caller that keeps the slice past a
    mutation of the list.
  - Probe: `conformance/probes/list_slice_borrows_the_list.mojo`.
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
    `mir::lower_whole_drops` writes the destructor's call with no raise
    contract, and Pliron rejects a raising destructor at the drop.
  - Reject the declaration at the check, with the pin's message.
  - Found while landing R1043 (2026-10-16).
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

- [ ] **R607 The bundled publics keep bare linked names the compiler
  recognizes by spelling**

  The library's `List`, `range`, `Writer`, `Slice`, and their kin link
  under their bare names, and the checker finds them by that spelling,
  while upstream names every declaration by its module
  (`std.collections.list.List`).
  - `implicit_public_identity` and `builtin_module_exports`
    (`crates/mojito-module/src/module.rs`) keep the names bare; checker
    sites match `"List"`, `"String"`, `"Slice"`, and others directly.
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

- [ ] **R1051 `Sized` and `SizedRaising` are compiler-known, where
  upstream's library declares them**

  Upstream declares `trait Sized` and `trait SizedRaising`, each with a
  `__len__` requirement, in `std/builtin/len.mojo`; Mojito knows both by
  name and declares no requirement, so `def f[T: Sized](x: T)` returning
  `x.__len__()` fails with "type 'T' has no method '__len__'".
  - That rejects valid Mojo, and R988's library `len[T: Sized]` cannot be
    written until it is fixed.
  - Declare both in the bundled std, export them as builtins, and retire
    `"Sized"`/`"SizedRaising"` from `conformance::COMPILER_TRAITS` and
    `checker::builtins::has_len_bound`, following R705's model.
  - Every std struct with `__len__` lists `Sized`, as the pin's do; check
    whether the compiler-modelled `SIMD` and `StringLiteral` still conform.
  - Done when the probe above runs and every `assets/ok` fixture using
    `Sized` runs unchanged.
  - Found while planning R1021 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R988 `len` is a checker builtin, where upstream's is a library
  function**

  The pin declares `@inline(.always) def len[T: Sized](value: T) -> Int`
  and its `SizedRaising` overload in `builtin/len.mojo`, and Mojito
  checks `len(x)` as a builtin `call` the elaborator dispatches.
  - Declare both in the bundled std and resolve `len(x)` as an ordinary
    overloaded call, keeping the builtin only where a pack length needs
    it (R804).
  - Done when `len(xs)` over a `List` lowers to a call of the library
    `len` and every fixture's output is unchanged.
  - Found while landing R939 (2026-10-15).
  - Depends on nothing.
  - Model: Fable, Not Planned.

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

- [ ] **R743 A runtime variadic pack is a builtin type, where upstream's is
  the library's `VariadicListMem`**

  A `*xs: T` collector is the compiler's `$variadic[T]`
  (`TyKind::VariadicPack`), while upstream lowers it to the nominal
  `VariadicListMem[T, origin, is_owned]` with its `_VariadicListMemIter`.
  - A borrowed loop over a pack is upstream's iterator inlined in MIR: the
    `PackCursor` in `crates/mojito-mir/src/mir/lower_stmt.rs`
    (`normalize_iterator`, `emit_has_next`, `emit_next`).
  - Owned iteration (`for var v in xs^`) keeps the builtin consuming
    `iter.next` on the VM and natively.
  - Closing it declares the pack in the bundled
    `stdlib/std/builtin/variadics.mojo`, beside `ParameterList`, and deletes
    the cursor and the pack arms of `HasNext`/`Next`.
  - Found while landing R734 (2026-10-13).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R1072 Writing a homogeneous collector is rejected**

  Problem: `print(xs)` over `*xs: Int` reports that "an element of
  'values' with type '$variadic[Int]' does not conform to trait
  'Writable'", where the pin prints `(1, 2, 3)`.
  - Upstream's `VariadicList` is `Writable` when its element is, through
    its `write_to` and `write_repr_to`.
  - Once R743 makes the collector the nominal `VariadicList`, this is
    declaring those two methods on it.
  - Found while planning R1040 (2026-10-16).
  - Depends on R743.
  - Model: Opus, Not Planned.

- [ ] **R1073 A run-time pack's element reads and destruction are native
  backend code**

  The native backend reads a `VariadicPack` through its `{ elements, len }`
  descriptor and destroys an owned one in a backend-emitted loop, where
  upstream runs `VariadicList.__len__`, `__getitem__`, and `__deinit__`.
  - The code lives in `crates/mojito-pliron/src/lower/iter.rs`
    (`pack_length`, `pack_element_address`, `advance_variadic_list`) and
    `lower/drops.rs` (`emit_variadic_list_drop`); the VM's
    `drop_owned_pack` is its counterpart.
  - Once R743 declares the nominal `VariadicList` over the same layout,
    these become calls of its library methods and the backend arms go.
  - Probe: an owned `for v in xs^` over a run-time pack normalizes the
    pack in place (`iter.init` with no `prepare`), its slot holding a
    backend cursor, where every nominal `__iter__` now makes an iterator
    slot of its own type (R1088); the nominal `VariadicList` splits too.
  - Probe: the read of a collector a call spreads (`show(*args)`) is a
    `var.use copy` of the collector's slot, which `lower_copies_and_moves`
    keeps a use for the elaborator to expand (`expand_pack_spreads`),
    where upstream passes the `VariadicList` by reference and copies
    nothing (found while landing R1046).
  - Found while landing R1040 (2026-10-16).
  - Depends on R743.
  - Model: Opus, Not Planned.

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
    conventions, but no result type and no `ParamDecl`s.
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
  - Delete the syntax half of `with_stmt::splice_with_desugars` if
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
  - The overloads reject a float or `Bool` lane with `comptime assert`,
    which Mojito does not parse (R74).
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
    (`Checker::reads_raw_parameter_list`), as `size` already is.
  - Upstream's index parameter is `idx: SIMDLength`, which Mojito lacks, so
    the port spells it `Int` and records that in the module header.
  - A runtime subscript already calls `__getitem__`, and the length is
    already `size`.
  - Found while landing R494 (2026-10-13).
  - Depends on R801, which gives a `comptime` value member parameters.
  - Model: Fable, Planned.

- [ ] **R805 A compile-time read of a type pack is a checker rule, not a
  `TypeList` member**

  Problem: `len(Ts)`, `Ts.length`, and `Ts[i]` over a type pack are
  recorded by the checker as pack queries, where upstream answers them
  through `TypeList.size` and `__getitem_param__` on a library struct.
  - Mojito has no bundled `TypeList`: the checker builds it
    (`Checker::typelist_receiver`, `unbound_pack_named`, the type-pack arms
    of `pack_length_binder`).
  - A value pack's reads already go through the bundled `ParameterList`
    (R494), which is the model to follow.
  - Found while landing R494 (2026-10-13).
  - Depends on R801, which gives a `comptime` value member parameters.
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
  - Depends on nothing.
  - Model: Fable, Planned.

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

- [ ] **R538 Storage paths for reference-valued elements outlive the last spelling of one**

  Problem: the checker, MIR, VM, and native backend keep paths for an element whose type is itself `ref[o] T`, though `List[ref[o] T]` and `Tuple[ref[o] T]` are now rejected at parse, as at the pin.
  - Sites: `infer_list_construction` and the `Tuple` arm of `infer_storage_value` in the checker, the element-slot comments in `crates/mojito-mir/src/mir/lower_stmt.rs` and `mir/verify/instr.rs`, `MirInstr`'s element-handle note in `mir.rs`, and `lower_make_ref` in `crates/mojito-pliron/src/lower/ctors.rs`.
  - Most of these also serve a struct with a direct `ref` field as an element, so delete only a branch whose sole trigger is a top-level reference element.
  - A bare `List[ref Int]` still parses and is rejected by the checker, so a `Ty::Ref` element type can still be formed until it is rejected.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R695 A list or set display is filled by `append`/`add`, not built
  by its `__list_literal__` / `__set_literal__` constructor**

  Problem: upstream lowers `[a, b]` in a `List` context and `{a, b}` as one
  variadic literal-constructor call, where Mojito constructs an empty
  collection and inserts each element; only `Array` displays get the
  constructor call.
  - The checker's `record_collection_construction`
    (`crates/mojito-checker/src/checker/inference.rs`) gives every
    non-`Array` display a `ConstructCollection` insert plan;
    `record_array_literal_construction` gives `Array` its constructor.
  - Lever: record the literal constructor for `List` and `Set` displays
    too, keep the insert plan for comprehensions, and lower them as the
    `Array` path does.
  - Consumers to follow: `origins/actuals.rs`, `declarations.rs`,
    `checked.rs`, and `mono/substitute.rs::list_literal_constructor`.
  - Semantics already match since R686: the elements are read before the
    collection is built.
  - Found while landing R686 (2026-10-13).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R583 The VM resolves a callee by name at run time, scanning every
  concrete function**

  Problem: `Prog::{overload_name, runtime_method_name, constructor_name,
  index_of}` (`mojito-vm/src/backend/vm.rs`) walk the whole concrete
  program for each dispatch they resolve, while upstream resolves every
  call at compile time and the VM runs concrete MIR the elaborator already
  resolved.
  - The VM's `CallableLookup` implementation scans `mir.functions` for both
    the exact name and the overload family.
  - The lever is to have the elaborator record each concrete call's
    target, so the VM resolves no name; failing that, an index by name.
  - Found while landing R580 (2026-10-10), which moved the elaborator's
    resolution onto by-name lookups.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R728 A struct infix operator's concrete call says it cannot
  raise**

  Problem: `native::mono` rewrites a struct `BinOp` to its dunder's
  `MethodCall` through `dunder_method_call` (`mono/equiv.rs`) with
  `raises: None`, even over a raising `__add__`.
  - Mojo's `a + b` is the dunder's call, and that call carries its effect.
  - Execution is right only because the VM propagates through Rust's `?`
    and native lowering reads the effect from the callee's declaration.
  - Fix: lower a struct operator to the dunder's call carrying the
    checker's effect, by giving MIR `BinOp` a `raises` contract or by
    making the struct operator a `MethodCall` before mono.
  - Found while landing R720 (2026-10-13).
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

- [ ] **R875 The move and initialization checks read a lowered CFG where
  `CheckLifetimes` walks structured regions**

  `CheckLifetimes` joins an `if`'s arms and iterates a loop to a fixpoint
  (`checkIfOp`, `checkLoopOp`), and Mojito's checks run block dataflow over
  the CFG the transitional pass builds.
  - Stage 6 of `docs/structured-control-flow-plan.md`.
  - The lever is the move/initialization scan in `crates/mojito-analysis`.
  - The state at a `Break`/`Continue` flows to its labelled loop.
  - After it lands, `lower-control-flow` still runs before the borrow
    checks.
  - `mir::lower_whole_drops` keeps a definite-initialization fold of its
    own (whole, missing parts, empty) that follows `try` regions; once
    this lands it reads this state instead.
  - The entry's planning session may split it by analysis if one session
    cannot land it, filing the rest as chained entries.
  - Depends on R873, R874.
  - Model: Fable, Planned.

- [ ] **R876 The borrow checks read a lowered CFG where `CheckLifetimes`
  walks structured regions**

  Loans, register loans, and interior borrows are block dataflow, and
  upstream checks them on the structured IR.
  - Stage 7 of `docs/structured-control-flow-plan.md`.
  - The lever is `loans`, `register_loans`, and `interior` in
    `crates/mojito-analysis`.
  - After it lands, `lower-control-flow` runs after the ownership checks,
    before drop elaboration.
  - The entry's planning session may split it by analysis if one session
    cannot land it, filing the rest as chained entries.
  - Depends on R875.
  - Model: Fable, Planned.

- [ ] **R877 Drop elaboration places drops on CFG edges where
  `DestructorInsertion` places them in structured regions**

  `analysis/drops.rs` puts drops on edges and splits critical edges, and
  upstream's regions have no edges.
  - Stage 8 of `docs/structured-control-flow-plan.md`.
  - A drop goes before a region terminator, at a region's entry, or after
    the op.
  - Critical-edge splitting is deleted.
  - After it lands, `lower-control-flow` runs after drop elaboration,
    before field drops, store drops, and copy elision.
  - The entry's planning session may split it if one session cannot land
    it, filing the rest as chained entries.
  - Depends on R876.
  - Model: Fable, Planned.

- [ ] **R878 Field drops, store drops, and copy elision run on a lowered
  CFG rather than on regions**

  These passes follow drop elaboration and still read the CFG the
  transitional pass builds.
  - Stage 9 of `docs/structured-control-flow-plan.md`.
  - The lever is `field_drops`, `store_drops`, and `copy_elision` in
    `crates/mojito-analysis`.
  - After it lands, `lower-control-flow` runs at the start of the
    pre-elaboration pipeline.
  - The entry's planning session may split it by pass if one session
    cannot land it, filing the rest as chained entries.
  - Depends on R877.
  - Model: Opus, Planned.

- [ ] **R879 The pre-elaboration pipeline and the package hold CFG bodies,
  and `canonicalize` simplifies blocks `hlcf` would not have**

  Upstream's generator `Canonicalizer` applies `hlcf`'s structural patterns
  (`HLCFOpsCanonicalizers.cpp`), and R836's rewrites thread and merge
  blocks that structured MIR does not have.
  - Stage 10 of `docs/structured-control-flow-plan.md`.
  - `canonicalize` gains `RemoveStaticCondition`, `HoistYieldResults`,
    `RemoveUnusedResults`, `HoistUnconditionalReturn`,
    `HoistConditionalReturn`, `RemoveDeadLoop`, and `checkTrivialBlocks`.
  - R836's pass-through threading, identical-successor fold, and
    single-predecessor merge are deleted, and their
    `tests/mir_passes/canonicalize/` cases are rewritten as `hlcf`-pattern
    cases.
  - `sccp` walks regions.
  - Package bodies are structured, so `PACKAGE_FORMAT` is bumped.
  - Updates `docs/architecture.md` §Pre-Elaboration Pipeline and
    `docs/features.md`'s pipeline row.
  - After it lands, `lower-control-flow` runs at the elaborator's entry, as
    each template is materialized.
  - Depends on R878.
  - Model: Fable, Planned.

- [ ] **R880 The elaborator unrolls `comptime for` by recovering its body
  through dominators where upstream clones the op's region**

  `processComptimeForOp` makes one labelled `hlcf.loop` per iteration from
  the op's region, and `mono/unroll.rs` finds the body as the blocks its
  `body` target dominates.
  - Stage 11 of `docs/structured-control-flow-plan.md`.
  - Instantiation, substitution, `ComptimeIf` (keep the taken arm), and
    `ComptimeFor` work on regions, with `simplifyParamForLoop`'s folding.
  - `unroll.rs`'s `loop_body` and `outermost_loop` are deleted.
  - A CTFE fragment is lowered before `call_concrete`.
  - After it lands, `lower-control-flow` runs at the end of elaboration.
  - Depends on R879.
  - Model: Fable, Planned.

- [ ] **R881 The post-elaboration pipeline runs on a CFG and has no
  counterpart of `SimplifyCF`'s trivial-loop removal**

  Upstream runs `SimplifyCF` only after elaboration, on functions, and
  Mojito's post-elaboration `sccp`/`canonicalize` read lowered blocks.
  - Stage 12 of `docs/structured-control-flow-plan.md`.
  - `sccp`/`canonicalize` run on regions in `post_elaboration.rs`.
  - Add `SimplifyCF`'s trivial-loop removal (`lib/Transforms/SimplifyCF.cpp`).
  - After it lands, `lower-control-flow` runs at the backend boundary
    (`src/compiler.rs`, `src/artifact.rs`, the Pliron entry).
  - Depends on R880.
  - Model: Opus, Planned.

- [ ] **R882 The VM executes only CFG blocks where upstream's interpreter
  runs `hlcf` ops**

  Every `hlcf` parent op implements `InterpreterOpInterface`, and the VM
  needs the transitional lowering before it can run a body.
  - Stage 13 of `docs/structured-control-flow-plan.md`.
  - Generalize `run_region`/`exec_try`'s `Flow` in
    `backend/vm/exec.rs` over every region op.
  - The VM path and CTFE stop lowering, so `lower-control-flow` runs only
    for Pliron.
  - Depends on R881.
  - Model: Opus, Planned.

- [ ] **R883 Pliron lowers a MIR CFG where `LowerControlFlow` lowers
  structured ops, so `lower-control-flow` and the CFG terminators survive**

  Upstream's first CFG is the one `LowerControlFlow` builds in LLVM, and
  Mojito's backend still reads `Jump`/`Branch`.
  - Stage 14 of `docs/structured-control-flow-plan.md`.
  - `crates/mojito-pliron`'s `lower/term.rs` and `lower/try_flow.rs` lower
    region ops to LLVM blocks.
  - The transitional pass and `Jump`, `Branch`, `ComptimeBranch`, the
    `ComptimeFor` terminator, `FallOff`, and `EscapeJump` are deleted.
  - Depends on R882.
  - Model: Opus, Planned.

- [ ] **R884 A `finally` is run by the VM on each exit where upstream
  copies it before each exiting terminator**

  `LowerSemanticCF`'s `lowerTryFinally` copies a `finally` body before every
  terminator that leaves the `try`, and Mojito keeps one `finalbody` region
  the VM runs dynamically.
  - Gap kept open by `docs/structured-control-flow-plan.md`.
  - Related to R829 and R830, without depending on them.
  - Once `try` parts are single regions, the copy can be made in MIR
    lowering, before the ownership checks.
  - Depends on R874.
  - Model: Fable, Planned.

- [ ] **R886 Structured region ops carry no values across a region
  boundary, where upstream's `hlcf` ops pass operands and results**

  Upstream's `hlcf.loop` and `hlcf.comptime.for` carry loop operands and
  results, and `hlcf.break`, `hlcf.continue`, and `if.elifcond.yield` pass
  values; Mojito's `Loop`, `ComptimeFor`, `Break`, `Continue`, and
  `ElifCond` carry none, since MIR blocks have no arguments.
  - Values cross those boundaries in slots, as they do in today's CFG.
  - `If`/`ComptimeIf` results already travel through `Yield`/
    `ComptimeYield`; `lower-control-flow` stores them in slots.
  - Register promotion is the lever: values the producers keep in slots
    become operands once a promoted register can cross a region.
  - Depends on R787, R872.
  - Model: Fable, Not Planned.

- [ ] **R890 A `for` over a raising iterator tests a `TryNext` flag where
  upstream calls `__next__` in a `try` whose handler breaks to the loop's
  `else`**

  `HirInstr::TryNext` and `MirInstr::TryNext` catch the exhaustion type
  inside one instruction and write a `Bool` the loop's opening `if` tests.
  - Upstream's `emitForStmt` wraps the `__next__` call in an error region
    that leaves through the loop's `else` (`lit.loop.break.else`).
  - Comprehension loops (`lower_expr/ctrl.rs`) share the shape.
  - With single-block `try` parts the call can be a `Try` whose handler
    breaks, and `TryNext` can go.
  - Found while landing R872 (2026-10-14).
  - Depends on R874.
  - Model: Opus, Not Planned.

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

- [ ] **R905 A `break`/`continue` from a `try` nested in another `try`'s
  part, to a loop inside that outer part, is unsupported**

  The VM run fails with "try with break/continue crossing the try
  boundary", where the same Mojo program is valid.
  - The structured form is fine: the part ends in a labelled `break`
    naming the loop (R874).
  - The limit is in the lowered form: `lower-control-flow`'s
    `Lowering::leave` can only aim an `EscapeJump` at a function-level
    block, and this loop's blocks belong to the outer part.
  - Once the VM runs region ops (R882), the `break` leaves through the
    regions, as upstream's interpreter does, and no escape is needed.
  - Add an `assets/ok` fixture with a `break` and a `continue` across two
    `try`s, each with a `finally`.
  - Pliron keeps the limit until R883.
  - Found while landing R874 (2026-10-14).
  - Depends on R882.
  - Model: Opus, Not Planned.

- [ ] **R906 A `raise` is a mid-block MIR instruction, where upstream's
  `raise` ends its block**

  `MirInstr::Raise` is followed by its region's normal terminator
  (`jump`/`yield`), so every analysis special-cases it as "the rest of the
  block is unreachable".
  - The special cases: `moves.rs` (two), `interior.rs`, `verify/drops.rs`,
    and `register_loans.rs`.
  - Lever: a `raise` region terminator in HIR (`Terminator`) and MIR
    (`MirTerm`), lowered by `lower-control-flow`; the VM (`exec.rs`),
    Pliron (`lower/instr.rs`), and the MIR text format (a version bump)
    follow.
  - Dead statements after a `raise` are already erased
    (`CheckedTables::live_statements`).
  - Found while landing R889 (2026-10-14).
  - Depends on R889.
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

- [ ] **R1065 A static method's self-recursive call keeps the struct
  parameter it forwards to itself, where the pin removes it**

  `S[n].down(k)`, which returns `7` or `S[Self.n].down(k - 1)` (or
  `Self.down(k - 1)`), keeps `n`, where the pin emits one
  `S::down(…)_REMOVED_ARG` with no `n=` in its symbol.
  - R1028 discounts a self-recursive forward only through a call's or a
    resolved method call's own parameter arguments
    (`self_forwarded_parameters` in `analysis/unused_params.rs`).
  - A static call's spelled receiver (`S[Self.n]`) or `Self` forwards the
    struct's parameters instead, which `drop_receiver_arguments` in
    `mir/param_uses.rs` drops, so the walk should count a receiver argument
    naming the parameter itself as a self-forward too.
  - Add the case to `assets/ok/static_method_unread_owner_instances.mojo`.
  - Found while landing R1028 (2026-10-16).
  - Depends on nothing.
  - Model: Opus, Not Planned.

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

- [ ] **R809 `ParameterList.of` and `empty_of` are not ported**

  Problem: `comptime Ints = ParameterList.of[4, 5, 6]` is rejected, where
  the pin's `std/builtin/variadics.mojo` declares both aliases.
  - Upstream: `of[type: AnyType, //, *values: type] =
    ParameterList[type=type, values.values]`, and `empty_of[type] =
    Self.of[type=type]`.
  - Found while planning R493 (2026-10-13).
  - Depends on R806 (the alias read through the struct) and R807 (the raw
    `values` parameter).
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
  - Depends on R811 and R805 (a bundled `TypeList`).
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
  - Port `_InvalidTypeIndex`, `_get_type_index`, and `_check` verbatim once
    `comptime assert` parses.
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

- [ ] **R850 Float SIMD `//` and `%` are rejected**

  Problem: `Float32(7.5) // Float32(2.0)` fails with "operator '//' is not
  defined for Float32 and Float32", while the pin prints `3.0`, and `%`
  prints `1.5`.
  - Once the VM computes them, add them to `ct::float_lane_binop`, so
    `sccp` folds them too.
  - Found while landing R846 (2026-10-14).
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
