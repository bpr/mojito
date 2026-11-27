# Complete Call Applications: Staged Plan

**Status:** plan recorded 2026-11-26. B1a–B1d (R2625, R2626, R2627,
R2631) have landed, and B1e (R2640) is the last slice before B2 (R2628).
The scheduled tasks are on [`docs/roadmap.md`](roadmap.md)'s `pmir` track,
one entry per slice below, chained by their Depends bullets. This document
is their design record; update it as slices land.

**Scope rule (owner's triage, 2026-11-27).** B1's run filed more
prerequisites than it landed. Each was a rare origin shape that the
complete record had to support. From here, a slice covers the shapes the
corpus and the bundled library spell. If a shape the record cannot yet
write is rarer, the checker rejects it with an explicit error and
`docs/non-goals.md` lists it ("Rare origin-binder shapes stay
rejected"). The slice does not file it as a new prerequisite. A slice
never adds a Depends bullet for a shape no corpus fixture or library
function needs.

**Goal.** Every call in MIR names its callee with the callee's complete
compile-time parameter application, and binds its operands by position, as
upstream's `kgen.call` does. The elaborator then instantiates a call by
substituting that application under the caller's bindings. It never infers
a binding from argument types.

## What Mojo does

- After the semantic check, a call names its callee generator with every
  compile-time parameter in declared order. That includes the inferred
  parameters, the binders `Some[Trait]` desugars to, and origins with their
  mutability. In `Mojo/test/kgen/lit-dialect/lit-attrs.mlir:58`,
  `kgen.call @lifetime_lower<:origin<true> #lit.any.origin>(%cst)` carries
  an origin parameter in the call's own list.
- A call's operands are positional. Keyword arguments and omitted defaults
  are resolved by sema's call binding, so no IR operand is a keyword.
- `ElaborateGenerators` instantiates a call by evaluating its parameter
  list under the enclosing instance's parameters (substitution plus the
  interpreter). It does not unify.

## What Mojito does

The facts below are as of `2e7247278`.

- **MIR `Call` and `MethodCall` carry `instantiated_args`.** These are the
  arguments the checker solved over the callee's whole declared list,
  origin binders and their `mut=` binders in place (R2350, MIR text schema
  1.205). A construction's struct prefix is the constructed instance's own
  arguments (`Checker::constructed_prefix`, B1b). The elaborator binds a
  closed `mut=` value from the record; it erases origins (`Bindings` keeps
  no origin map), so it binds nothing from an origin slot.
- **`Some[Trait]` binders are declared (B1a, landed).** Each is an
  infer-only binder of its own declaration, appended after the declared
  binders in argument order (`annotations::declare_existential`), and
  every reader uses the declared binder. The call solves it as any
  inferred binder.
- **Every call carries one argument per binder (B1c, B1d).** A
  subscript's accessor call included. No top-level argument is a
  placeholder (`ct_deferred`, `origin_unbound`), which the verifier
  enforces (B1e, R2639); one nested in a type argument still can be
  (R2640).
- **The elaborator re-derives bindings anyway.**
  - `Specializer::infer_call` (`native/mono/infer.rs`) seeds its bindings
    from `bind_instantiated_arguments`. That function skips every recorded
    argument that is still symbolic, i.e. one over the caller's own binders,
    with the note "unification leaves [it] as it stands".
  - `infer_call` then unifies every argument's type, the result type, a
    pack's overflow, and `**kwargs` elements against the declaration
    (`unify`, `unify_result`, `bind_pack`).
  - It rebuilds the instance's identity from those bindings
    (`own_arguments`, `mutability_arguments`, `push_sugar_arguments`).
  - `unify.rs` (1,200 lines) and `infer.rs` (1,091 lines) are this
    machinery.
- **What fails because of it.**
  - When unification's answer disagrees with the check's, elaboration fails
    with "conflicting solutions for `…`". R2440's two fixtures failed this
    way on a `Pointer` initializer's `mut` until B1b. There the record was
    wrong, not unification: the construction's `mut=` slot was worked out
    again from the origin's owner instead of read from the instance the
    check built.
  - When the identity it rebuilds differs from the one a type spells, one
    instance gets two spellings. R2167 records `Span[…, Pointer[UInt8, …]]`
    spelled both with and without its `mut` argument.
- **Keyword operands survive into MIR.** `kwargs` remain on `Call`,
  `MethodCall`, and `CallIndirect`. The elaborator, the VM, and Pliron each
  match them to parameter slots (`mojito_ast::call::match_call_slots`).
  `mir::call_defaults::fill_omitted_arguments` already writes omitted
  defaults into the call after lowering, but it leaves keywords as
  keywords.
- **`conventions` is optional.** `Call::conventions` is "empty when the
  call records no checked binding; the loan check then reads the callee's
  declaration".

## The target invariant

A call to a generic callee carries one compile-time argument per declared
binder, in declared order, including origins, `mut=` binders, and
`Some[…]` binders. Each argument is closed, or open only over the caller's
own binders. The MIR verifier rejects a call that breaks this. Its operands
are positional, and it carries one convention per operand.

## Slices

Each slice moves Mojito toward upstream's call and lands on its own. They
are in dependency order.

### B1a. `Some[Trait]` parameters declare their binder (R2625, landed)

- The checker desugars each `Some[Trait]` parameter type to an inferred
  binder in the declaration's own `param_decls`, as upstream does. The
  parameter's type then names that declared binder.
- This deletes all four `starts_with("Some[")` tests. Each reader uses the
  declared binder instead.
- Acceptance: the `Some[…]` fixtures under `assets/ok` print as before, and
  `rg 'starts_with\("Some\['` finds nothing.

### B1b. Checked calls record the whole application (R2626, landed)

- Landed as two pieces. R2350 made the record the whole declared list,
  carried it in MIR (schema 1.205), and had the elaborator bind `mut`
  from it. R2626 then found the one wrong slot: a construction's struct
  prefix re-derived each `mut=` value from its origin's owner
  (`declared_call_arguments`' bracket branch), so `Pointer(_mlir_value=r)`
  over an immutable origin of a `var` recorded `mut = True`.
  `Checker::constructed_prefix` now takes the prefix from the constructed
  instance, as upstream's constructor applies the type it builds, for
  constructions (`bind_constructed_origins`) and conversions
  (`construction_arguments`). R2440's fixtures, and
  `origin_binder_decided_mutability`, run with the pipeline off.
- The rest of this section is the slice as planned.
- The checker's call record covers the whole declared list, origins and
  `mut=` binders in place, instead of the argument view. That record is the
  node facts `generic_instantiation`, `method_instantiation`, and
  `bound_instantiation`, which the seal turns into
  `SemanticAdjustment::InstantiatedArguments`
  (`mojito-checked/src/checked.rs`), plus a conversion's
  `ConversionInstantiatedArguments`. The check already solves origins and
  their mutability in the call's one solve; this slice records that
  solution.
- MIR lowering copies the record into `Call` / `MethodCall`
  `instantiated_args`, and the MIR text format carries it (schema bump).
- `bind_instantiated_arguments` binds origins and `mut` from the record.
  Unification stays as a fallback in this slice.
- Acceptance: R2440's two fixtures elaborate with
  `MOJITO_PRE_ELABORATION=0` if the conflict is the only obstacle. If it is
  not, the next obstacle is recorded in R2440.

### B1c. Every call carries one argument per binder (R2627, landed)

- `verify_application` (`verify/calls.rs`) rejects a `call`, `call.method`,
  `def[...]` bound call, or checker-instantiated `call.indirect` whose
  `instantiated_args` is not one argument per binder its callee declares.
  A call naming an instance, which declares none, carries none.
- The ~30 `instantiated_args: Vec::new()` sites were mostly not sites:
  drops, register ends, iteration, and closure canonicalization call
  callees that declare no binders of their own. The sites the rule reached
  were these families:
  - Constructions of a struct whose binders are all origins or infer-only
    (`StringSpan`, `Origin`, `_BytesIter`, user views) recorded no prefix
    (`Checker::record_construction_instantiation` returned early).
  - Tuple and `Array` displays carried none; the checker now records the
    written construction's application at the display
    (`Checker::record_display_application`).
  - An in-place dunder lowered through its contract dropped the
    application; `CheckedCallContract::instantiated_args` carries it.
  - A pack struct's copy or move carried none; it now restates the pack
    as its construction binds it (`declared_pack_arguments`, a computed
    list as its value).
  - The `**kwargs` collector's `_insert` and the nominal `String` wrap of a
    literal carry the literal's text, and a `StringLiteral` iterable
    materializes through `Checker::materialize_at`.
  - The elaborator's copies and moves named an instance but restated the
    template's arguments; `lower_copies_and_moves` takes `applied`.
- What remains is placeholder slots (B1e).

### B1d. A subscript call carries its application (R2631, landed)

- `MirSubscriptCall::instantiated_args` is filled from
  `CheckedCallContract::instantiated_args`, written in the MIR text when
  non-empty (schema 1.237), visited with the call's other types, and
  carried by the rewrites that turn a subscript into a call
  (`subscript_store_call`, `receiverless_subscript_call`). The elaborator
  clears it when it names the accessor's instance.
- `verify_subscript_call` calls `verify_application`.
- The rule reached one site: a tuple destructuring's element read, built
  from `CheckedTupleUnpackElement`, which recorded no application. The
  checker now records the element's index as the accessor's argument
  (`Checker::tuple_unpack_plan`). Every contract-built subscript already
  carried a complete application.
- `remove-unused-params` drops a removed binder's entry from a subscript's
  application in its analysis view, as it does for a method call.

### B1e. No argument is a placeholder (R2640; R2639, conversions, R2633, R2634, R2635, R2636, R2637, R2638, and R2646 landed)

- **Landed: the top-level rule (R2639).** The verifier rejects a
  top-level `origin_unbound` slot, and a top-level `ct_deferred` one
  unless it is the caller-open `$origin_mut` read
  (`Mutability::of_mut_argument`): it is neither closed nor open over the
  caller's binders, so B2 cannot substitute it. The finding is "call of
  'f' leaves its compile-time argument `o` a placeholder
  (origin_unbound)".
  - The probe found one stdlib cause: a list display at a `Span[T, _]`
    compile-time parameter (`polynomial_evaluate[[c0, c1]](x)` in
    `_log_base` and `_log_kernel`) left `$origin_coefficients` and
    `$mut_coefficients` unsolved. Its conversion's `ref [Self.origin]`
    source is compile-time memory, so the converted value is now typed
    over that memory's immutable origin
    (`Checker::implicit_parameter_application`), and
    `resolve_call_params` solves the open binders again from the
    converted value's type, as upstream unifies the parameter with it
    (`ComptimeOrigin` at the pin; `Origin::Static` here until R899).
  - A run-time argument whose view conversion the call's type binders
    decide (`",".join(xs)` over `elems: Span[T, _]`) left the origin and
    `mut` unsolved, since the conversion to `Span[T, _]` is not found
    while `T` is open: `resolve_call_params` now converts it again at the
    parameter the first solve substitutes and solves once more, as it
    does a compile-time one.
  - A copy or move constructor's call restates its copied register
    type's arguments, so a placeholder there is the type's: the rule
    skips it (R2640; `std_iter_map`'s lowered `_ArrayIter.__moveinit__`,
    R2653).
  - A call through a generic callable whose omitted callable default
    repeats an earlier omitted one (`fallback = primary` in
    `conformance/fixtures/generic_anonymous_callables.mojo`) left it
    deferred: the earlier binder's default value now joins the call's
    environment (`Checker::infer_use_params`), as a solved one does.
  - The two hand-written `inline-parametric` cases spell their callable
    binder as the nested body's `ct_func_symbol`. Beside a register that
    makes a closure of it, the pass binds the closure, which carries the
    body's captures (`capturing_callable_binder`; the pin rejects a
    capturing callable parameter, and R1947 retires the shape).
- A `ct_deferred`
  means one of three things today. A callee's own binder left unfilled is
  the placeholder. A `$origin_mut` read names the `mut` of an origin binder
  in the caller's scope that declares no `mut=` binder, which the caller's
  instance closes. A callable binder whose value arrives at run time is
  written by no current checker path.
- **Sizing probe.** In `verify_application`, after the count check, print
  one stderr line per top-level placeholder slot: the callee, the binder,
  the slot's kind, the calling function, and the whole application. Delete
  `target/mojito-cache` and run any fixture: the package build verifies
  every library body (about 30 s), so its lines are the stdlib list. A
  later run verifies only the bodies it reaches.
- **Landed: conversions and converted defaults.** These were about 95% of
  the stdlib's placeholder slots.
  - A conversion took its struct prefix from the expected type
    (`StringSpan[mut, _]` for a bare `StringSpan` parameter). It now takes
    it from the instance the selected constructor builds
    (`Checker::conversion_constructed`): the refined `out self` under the
    own origin solution (`StaticString`, `StringSpan[ImmOrigin(o)]`), and a
    view constructor's `ref [Self.origin]` source binds that origin to its
    place with its mutability (`instance_with_origin`), as an explicit
    `StringSpan(s)` does. The own origin binders the source binds
    (`other_origin`) reach the record too.
  - A converted default padded its constructor's own binders
    (`$value_lit`, `$value_data`) through
    `ParamDecl::constructor_call_arguments`. `CheckedConst::Construct`
    now carries the checker's recorded application (`instantiated_args`,
    MIR schema 1.238), and `omitted_argument` substitutes the call's
    bindings into it, as it does a `CheckedConst::Evaluate`'s.
- **Landed: a `mut=` slot an implicit binder of the caller decides
  (R2633).**
  - A `mut=` slot governed by `origin_of(self)`, a `ref` parameter's, or a
    bare `o: Origin`'s origin recorded the callee's own binder: every std
    iterator's `iterable_mut`, and `StringSpan(self)` and
    `Pointer[origin=origin_of(self)]` in a `ref self` method.
  - `declared_call_arguments`' `mut=` arm now records the caller-open
    read `Mutability::Param(id).mut_argument()`
    (`Checker::call_mutability_slot`): a receiver's or `ref` parameter's
    reference binder (`OriginParamId::reference_binder`), or the bare
    origin binder's own id.
  - Record only: `origin_mutability_slot` still spells types, so an
    instance type keeps the callee's deferred binder until R2517 (over
    R2516's declared `self_is_mut`) closes it.
  - A synthesized move's `_ArrayIter.__moveinit__` slot in
    `std_iter_map`'s `main` is not this cause: it restates a register type
    whose nested `_ArrayIter` keeps its own deferred `mut=` and
    `Array.__iter__`'s unrebased `origin_param(0)`, which `main` does not
    declare. A `$origin_mut:0` read there would name a binder `main`
    lacks, so it moved to R2517.
- **Landed: a receiver's own origin binder (R2634).**
  - `instantiate_method_generics` pushed the receiver's clause into the
    one solve only for a symbolic address space, so `peek[o: Origin](ref[o]
    self)` recorded `_` for `o` on every receiver form.
  - The receiver is now argument 0 of every method's solve, whatever its
    clause.
  - `reference_solve_pair` reads a temporary argument or receiver as its
    materialized slot (`solving_reference_actual`), lent immutably unless
    the clause is `mut`, as the pin binds `S().peek()` and `by(S())`. That
    also closed `by(S())` and `s.take(S())` over a free or method `ref[o]`
    parameter, which recorded `_` too.
- **Landed: a collector's origin (R2635).**
  - `build_variadic_list` wrote `_` as a `VariadicList`'s origin in its
    register type and in its constructor's application, in every
    homogeneous `*xs` call and the elaborator's list display: the
    `print("hi")` probe's `main` and 16 stdlib functions.
  - The list's register type and application, and a heterogeneous call's
    `VariadicPack` register type, are now over the element references'
    `Untracked { mutable: owned }` origin (`element_origin`), as upstream's
    constructor argument check fixes `Self.origin` from the array's
    element type.
  - Neither the argument check nor the loan check compares it with the
    callee's collector type, whose origin is the callee's own per-argument
    binder, so nothing rejects it.
- **Landed: a `comptime` alias's bracket hole (R2636).**
  - A call through `comptime plus_six = first[_, 6]` recorded the hole as
    `_`: the alias's symbol holds `first`'s whole declared list, the hole
    `Origin::Unbound` for each call to infer
    (`declared_symbol_arguments`), but `folded_call_arguments` composed
    it under a guard that expected only the bracket's bound slots, so it
    fell back to the symbol.
  - It now fills each slot the value's generator still declares open with
    the call's inferred argument, by binder id, and keeps each bound one.
  - A hole no argument binds (`g()` over `comptime g = f[_, 7]`) still
    records `_`: R2439's rule, which the pin rejects at the call.
- **Landed: an origin binder's `_mlir_origin` sibling (R2637).**
  - `ParamDecl::Origin::mlir_origin` records the raw binder a binder's
    declared type names as its `_mlir_origin` (an alias-typed `[o: O]`'s
    implicit `$_mlir_origin_o`, or a spelled `Origin[mut=m,
    _mlir_origin=r]`), MIR schema 1.239.
  - `declared_call_arguments` binds that raw binder, and its `mut=`
    binder, from the sibling's origin (`ParamDecl::raw_origin_source`).
  - A struct's or method's list of that shape is still rejected at its
    declaration (`docs/non-goals.md`).
- **Landed: a callable default over earlier binders (R2638).**
  - `omitted_callable_value` (`checker/declarations.rs`) now holds an
    omitted callable default at the call's binding of the earlier binders
    it reads: `twice[k]` at `k = 5` is `twice[5]`, over a caller's open
    `j` it is `twice[j]`, and a conditional default the call decides is
    its arm, one over the caller's open binders a `cond` over them.
  - An arm of that `cond` specializing over the caller's binders
    (`twice[j] if b else twice[0]`) is `#kgen.bind_params` over the
    function's symbol, which folds to the closed symbol once the instance
    binds `j` (R2646 landed).
  - A default reading a binder the use leaves unbound stays deferred.
- **What remains.** A placeholder nested in a type argument, which the
  top-level rule does not reach and B2 does (R2640).

### B2. The elaborator substitutes the record and never unifies (R2628)

- `infer_call` and `infer_receiver_call` bind every declared binder by
  substituting the call's record under the enclosing instance's bindings,
  including a record open over the caller's binders. The instance's
  identity is the substituted record.
- Delete the argument, result, pack-overflow, and keyword unification.
  Delete `push_sugar_arguments` / `push_existential_arguments` and the
  "conflicting solutions" errors. `unify.rs` keeps only what struct-instance
  keying still needs, and the slice records what that is here.
- Acceptance:
  - R2440's fixtures run with the pipeline off (true since B1b; they stay
    a regression probe).
  - Re-probe R2167 (its entry says how).
  - The `measure` counters do not grow
    (`scripts/instantiation-report --check` in the nightly gate).

### B3a. Every call carries its conventions (R2629)

- `Call::conventions` and `MethodCall::conventions` hold one convention per
  operand at every construction site. The loan check's fallback to the
  callee's declaration is deleted.
- Sites first: add the verifier check, list the sites, and fix them all.

### B3b. Calls are positional below lowering (R2630)

- `fill_omitted_arguments` already binds each lowered call against its
  declaration. It now also places each keyword operand in its parameter's
  position and builds a `**kwargs` collector, as it builds a `*args` one.
- Delete `kwargs` from `Call`, `MethodCall`, and `CallIndirect`. Delete the
  call matcher's uses in the elaborator, the VM, and Pliron. Bump the MIR
  text schema.
- Acceptance: `rg 'kwargs' crates/mojito-vm/src crates/mojito-pliron/src`
  finds no call-binding use.

## Out of scope

- **One compile-time evaluator** (redesign D). B2 substitutes recorded
  applications, but it does not change who evaluates a closed one.
- **Typed declaration identity** (R935, redesign E). Records keep naming
  callees by symbol string.
- **`CallIndirect` through a runtime function value.** It already carries
  `param_decls` and "complete checker-resolved generic arguments". A runtime
  function value is a generator over origins alone, which each call infers.
  B1c's check did not reach it, so it stays as it is.

## What would stop a slice

- A rare shape the record cannot write. The checker rejects it explicitly
  and `docs/non-goals.md` lists it (the scope rule above). It is not
  filed as a prerequisite.

- A call whose application the check cannot write without re-checking (a
  synthesized call with no checked counterpart). The slice then records the
  shape here and builds the application from the callee's declaration
  under the caller's bindings, never by unifying operand types.
- Instance growth. If substituting the record mints instances that
  unification merged, the `measure` ratchet shows it. The slice files the
  growth as the batch runner already does, and does not revert to
  unification.
