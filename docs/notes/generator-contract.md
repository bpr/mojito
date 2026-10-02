# The generator contract

**Status:** written 2026-10-01 as the gate for stage P2 of
[`docs/parametric-mir-plan.md`](../parametric-mir-plan.md). It says what a
checked generic body guarantees every instance, and where each check a clone
makes goes once the clone is gone. P2's first step landed the same day: a
generic struct's method with no compile-time construct is served by its
template's MIR on every plain-data instance. §What has landed says which rows
that settled and which it left.

A **generator** is a generic declaration's checked body in MIR, with its
parameters still symbolic: what `CompiledProgram::drop_elaborated_mir` holds
for a trait-bound `def` today, and what P2 makes of an ordinary generic
struct's methods. The **elaborator** is `native::mono`. An **instance** is the
concrete function the elaborator produces from a generator and a binding of
its parameters.

## The guarantee

> Substituting arguments that satisfy a generator's recorded obligations
> preserves typing, ownership, and effects. No source expression is inferred
> again and no overload is ranked again.

The argument, per property:

- **Typing.** Every fact in the generator either does not mention a
  parameter, or is a type over parameters that substitution rewrites
  (`types::replace_parameters`, `mono/substitute.rs:substitute_ty`), or is an
  operation whose meaning is read from its operand's substituted type. The
  third kind is listed operation by operation in §The inventory. Nothing
  else in a body may depend on the instance.
- **Ownership.** Last uses, transfers, loans, and drop points are decided
  once, on the symbolic body, by the checker's origin analysis and by
  ownership analysis and drop elaboration over MIR. Substitution resolves
  what a `drop.var` or a copying `var.use` *runs* at the concrete type. It
  never moves one. A parameter is treated as the most demanding type its
  bounds allow, so a concrete type can only need less.
- **Effects.** A callable's raise contract, transfer summary, and
  call-through residue are part of its checked signature. An instance's
  effects are the generator's, minus sources that vanish because a
  substituted type carries no loan. They never grow.

**"Check once"** means one semantic checking process per declaration, not one
traversal. Effect summaries and loop dataflow still iterate inside it.

**Binding once.** A call inside a generic body is bound when the body is
checked. `pick(x)` with `x: T` selects `pick[T]` for every instance
(instantiation note, §Overload selection is bound once). The same holds for
an `@implicit` conversion and for a constructor family: the pin binds both on
the declaration (§Pin verdicts). An instance may *resolve a witness* — the
member of the concrete type that satisfies a requirement the bound named —
but it never ranks candidates.

## What a generator names

Each part is carried by a type that exists. The contract adds no parallel
vocabulary.

| What | Carried by | In MIR today |
|---|---|---|
| Binders | `ParamDecl::{Type, Value}` with its `ParamId`; uses are `Ty::Param { binder: ParamRef }` and `ParamExpr` over `ParamRef` | `MirFunctionDeclaration.param_decls`, `MirStructDeclaration.param_decls` |
| Assumptions a bound supplies | `ParamDecl::Type.bounds`, `callable_bound`, and `constraints: Vec<GenericConstraint>` | Yes, serialized with the declaration |
| Availability of a conditional member | `GenericConstraint` (the checker's `MethodSig.availability`) | **No.** See §Availability |
| Value propositions an instance owes | `ParamConstraint`: a `Bool` `ParamExpr` with its location and message, decided by `ParamConstraint::verdict` into a `ConstraintVerdict` | Not yet used above the waist; see §Predicates |
| Selected calls | Callable identity from `mojito-symbol`: the lowered symbol on `Call.func`, `MethodCall.resolved`, `CallIndirect.resolved`, `GetIter.prepare`, and the iterator call of `TryNext` | Yes |
| Witness requests | The abstract receiver `symbol::TRAIT_DISPATCH` (`__trait_dispatch.copy$ov$…`) on those same fields, and type-directed operations (§The inventory) | Yes |
| Compile-time arguments of a call | `MirParamArg`, binder-keyed, and the spelled receiver type of a static call (`MirInstr::Call::receiver`) | Yes |
| Lifecycle requirements | `var.use` modes, `CopyValue`, `DropVar`, `DropPlace`, `ConsumeVar`, `ConsumePlace` over a symbolic type | Yes |
| Origin and loan facts of a body | `EstablishLoans`, `InvalidateInteriors`, `arg_places`, `capture_accesses`, `MethodCall.reference_result` | Yes |
| Transfer and call-through summaries | `mojito_checked::checked::{TransferEffect, CallThroughEffect}`, keyed by lowered symbol | **No.** Checker tables only |
| Provenance | Source and module ids on every MIR location (`AGENTS.md` invariant 7) | Yes. Instantiation provenance is the instance symbol only |

Two vocabularies describe a predicate, and both stay, each for its own
operand kind:

- `GenericConstraint` speaks about types: conformance, pack predicates,
  trivial lifecycle, and comparisons of `ConstraintOperand`s. `where`
  clauses are stored in it, and MIR text already serializes it.
- `ParamConstraint` speaks about values: a canonical `ParamExpr` of type
  `Bool`. It is the form for a predicate over value binders.

A generator obligation is one or the other. `TemplateObligation` in
`mojito-checked/src/templates.rs` is neither: it names a *kind of re-check a
derivation performs*, not a proposition, and it is deleted with derivation.
The table in §The inventory says where each of its variants goes.

## The inventory

The source is the instantiation note's
[§What an instance still owes](instantiation-from-template.md#what-an-instance-still-owes),
by its numbers. That list is the migration inventory, not the specification:
most of it describes how to rebuild a clone's facts, and that work has no
successor.

Classes:

- **D** — proved when the declaration is checked. An instance owes nothing.
- **B** — an assumption a bound supplies. The bound is checked where the
  generator is applied, and the body uses it freely.
- **M** — an operation or summary the generator keeps, which the elaborator
  resolves from substituted types.
- **P** — a predicate checked when the generator is instantiated.
- **X** — clone bookkeeping. It disappears with the clone.

| # | Obligation | Class | Where it goes |
|---|---|---|---|
| 1 | Substitution | M, X | `Bindings` keyed by `ParamRef`. Reading `Self.T` from the resolved receiver is X: the elaborator binds the owner's parameters from the receiver's type (`Bindings.self_instance`). |
| 2 | `rebind` equalities | P; D | The equality of operand and target is a predicate over types. The pin checks it at instantiation ("function instantiation failed"). The by-value selection is D. |
| 3 | Implicit copies | D + M | The pin rejects a copy of a value whose type is not provably `ImplicitlyCopyable` on the declaration, and so does Mojito's template check. What remains is the copy itself: `var.use { mode: copy }` and `CopyValue`, run at the concrete type. |
| 4 | Local renumbering | X | A generator's locals are its own. |
| 5 | Direct calls: does a clone exist | X | The call names the callee and its compile-time arguments (`MirParamArg`). The elaborator enqueues the instance. |
| 6 | Built-in `len` | B → M | `Sized` supplies it. The call is a witness request on the operand's type. |
| 7 | Closed method calls | D; X; P | Selection is D. Retargeting to the clone member, and the rule that an overloaded family declares closed parameters so two rankings agree, are X: there is one ranking. A callee's `where` clause is P (§Availability). |
| 8 | Struct applications | X | The elaborator discovers a struct instance from the substituted types it meets (`discover_structs`), not from a recorded list. |
| 9 | Effect summaries stay empty | M | Summaries belong to the callee's signature. A generator's callee is another generator or a concrete function, and neither has a per-clone summary that can grow. |
| 10 | `Movable` at a `^` transfer | D | The pin rejects the transfer on the declaration unless the bound proves `Movable`, and so does Mojito (`Checker::bounds_prove_movable`). Nothing is owed. |
| 11 | Deletability | D + M | Whether a binding is deletable or linear is decided from the bounds, and the pin reports an abandoned value on the declaration. The destructor a `DropVar` runs is resolved from the substituted type. A type built over a parameter (`List[T]`) answers from its own conformance, which is a witness lookup. |
| 12 | Plain-data arguments | X | This is a limit of derivation, not a language rule: a template records no effect for a store of a symbolic value, so derivation refuses an instance that would have one. It is replaced by 14's rule. |
| 13 | Reference calls | D; M; X | The reference's origin and mutability are D. Whether a by-value read of the result copies is M, read from the referent's substituted type. Retargeting is X. |
| 14 | Replayed transfers | M | The summary is the generator's. An instance keeps a source while the substituted type of its root may carry a loan and drops it otherwise (`loan_free`). The escape verdict is monotone in the sources, so it is D. A latent effect — an outward store of a symbolic value — is part of the summary, published where the substituted type carries a loan. |
| 15 | Operators | B → M | `binary { resolved: absent }` over a parameter type: the elaborator selects the dunder from the substituted operand type. The result type is the one the requirement declares, so "the result must still be the type the template kept" is D. The operand copy a consuming dunder makes is 3. The conversion of an operand is 20. |
| 16 | Bound dispatches | B → M; X | `MethodCall.resolved` names `__trait_dispatch.<requirement>`, and the elaborator takes the witness from the concrete type's conformance (`traits.rs:requirement_witnesses`). It is never ranked. The witness-shape restrictions and per-call clone naming are X. A witness's availability is P. |
| 17 | Bound builtins | B → M | `hasher.update(x)`, `writer.write(x)`, `print`: type-directed calls resolved on the substituted type. `Hashable` and `Writable` come from the bound. A pack element is proved by the pack's bound. |
| 18 | Constructions | D; X; P | The member is selected on the declaration. Taking the instance's `__init__` clone is X. The member's `where` clause is P. |
| 19 | Call-through residues | M | A residue names a parameter slot and signature places, no type. It is the generator's, unchanged. The contract of a call through a callable parameter is the parameter's own. |
| 20 | Implicit conversions | D | The pin selects the `@implicit` constructor on the declaration: with `Int` and `Self.T` overloads, `Wrapper[Self.T] = self.item` picks the `Self.T` one in every instance. The generator records the selected constructor as an ordinary call, so a method its template serves agrees. A method that still clones repeats the selection per clone and reports an ambiguity the pin does not (§Pin verdicts, roadmap §3). |
| 21 | Iteration protocols | B → M; D | `GetIter.prepare` and the `TryNext` call name the chain. The elaborator resolves each step on the concrete type (`mono/infer.rs:rewrite_get_iter`). The checks on a linear element are D. |
| 22 | Truthiness conditions | B → M | `Boolable` supplies it. The mark is a conversion call on the condition, resolved on the substituted type. A condition that becomes `Bool` needs no call. |
| 23 | Tuple unpackings | M; X | The plan is rebuilt from the substituted value type. Naming the generated `Tuple$tN` for a public one is X until struct generators (P3d). |
| 24 | Comprehension binders | M; D; X | The clause's protocol is 21. Droppability is 11. Numbering is 4. |
| 25 | `with` desugars | X | The desugar is lowered once, into the generator. |
| 26 | Nested `def` declarations | M; D | Signatures and capture types substitute. A capture's capability (`ImplicitlyCopyable` for `var item`, `Movable` for `var item^`) is demanded of the bound, like 3 and 10. |
| 27 | Lane shapes | P; M | `to_bits` no narrower than the lane, and no `bool` lane in a `cast`, are predicates over value binders: `ParamConstraint`s. The dtype and width substitute. This row is P3c's, listed so the form is fixed now. |
| 28 | Bound conversions | B → M | `Int(x)` over `T: Intable` is `call { func: Int }` on a parameter-typed operand, resolved on the substituted type. A witness that raises would add an effect, so the requirement's signature decides the effect and a raising witness does not satisfy it. |
| — | The declaration's own bounds and `where` clauses | B, P | Checked where the generator is applied: at the call, for a call the checker sees, and by the elaborator for an application it discovers. |

Read by class:

- **Nothing is left for an instance to infer.** Every D row is a decision
  the symbolic check already makes or, in rows 10 and 20, one it must start
  making.
- **The predicates are few**: `rebind` equality (2), availability and
  `where` clauses (7, 16, 18, last row), and lane constraints (27).
- **The operations are witness lookups and lifecycle glue**, and the erased
  trait-bound path already carries each of them in MIR. A trait-bound `def`
  compiled today shows `var.use { mode: copy }`, `drop.var`, `binary {
  resolved: absent }`, `call { func: Int }`, `iter.init { prepare: […] }`,
  and `literal.materialize` over `param { owner, slot }` types
  (`mojito emit-mir`).
- **Two summaries are not in MIR**: transfers (14) and call-throughs (19).
  They live in checker tables. That is sufficient while the checker runs in
  every compilation. A package (§Package metadata) must serialize them.

### How `TemplateObligation` maps

| Variant | Rows | Fate |
|---|---|---|
| `DeclarationConstraints` | last | Stays, as `GenericConstraint`s on the declaration |
| `RebindEqualities` | 2 | Becomes an instantiation-time predicate |
| `ImplicitCopies` | 3 | Deleted: proved on the declaration |
| `PlainDataArguments` | 12 | Deleted: derivation limit |
| `Movable` | 10 | Deleted once the declaration demands the bound |
| `Deletability` | 11 | Deleted: proved on the declaration, resolved by the drop |
| `ReferenceResultReads` | 13 | Deleted: the read resolves on the referent type |
| `ReplayedTransfers` | 14 | Becomes the summary's substitution rule |
| `ConstructorSelection` | 18 | Deleted: selected on the declaration |
| `CallThroughResidue` | 19 | Deleted: the summary is unchanged |
| `ImplicitConversions` | 20 | Deleted once the declaration binds the conversion |
| `PrintableArguments` | 17 | Deleted: the bound supplies `Writable` |

## Predicates

An instantiation-time predicate is evaluated by the elaborator when it
materializes an instance, with the instance's `Bindings`.

- A proven predicate costs nothing.
- A disproven predicate on a *demanded* instance is a compile error that
  names the generator, the instance, and the clause's location and message.
  The checker reports the same failure earlier for every application it
  sees. The elaborator's check is what makes an imported package safe, and
  what covers an application only the elaborator discovers.
- A disproven availability predicate on a *discovered* member means the
  member is not part of the instance. It is skipped, silently and on
  purpose (§Availability).
- A predicate the bindings do not decide (`ConstraintVerdict::Residual`) in
  a concrete instance is an invariant violation: concrete bindings close
  every operand.

## Availability: the `speculative` audit

What the code does today (`crates/mojito-native/src/native/mono.rs`,
`mono/specializer.rs`):

- `Specializer::discover_structs`' eager walk enqueues, for each concrete
  struct instance, every overload of `__init__`, `__copyinit__`,
  `__moveinit__`, and `__deinit__`. Lowering composes lifecycle symbols by
  name, so they must exist whether or not a call names them.
- `__init__` overloads are enqueued through `enqueue_speculative`, which
  puts the instance name in `Specializer::speculative`. The other three are
  firm.
- A call site's `enqueue` of the same instance removes it from the set. The
  instance is then firm.
- In `Specializer::run`, a firm instance that fails to materialize fails
  the compilation. A speculative one is rolled back: its functions and
  declarations are truncated and its key is forgotten.
- "Fails to materialize" is any `MonoError`, including the per-instance
  concrete check (`mono/symbolic.rs:ensure_concrete_function`).

Why it exists: a member-level `where` clause is the checker's
`MethodSig.availability`. It does not reach `MirFunctionDeclaration`. Only a
binder's own `ParamDecl::Type.constraints` do. So `Slot[NoDefault]`'s
`__init__(out self) where conforms_to(Self.T, Defaultable)` looks, in MIR,
like any other constructor, and the walk cannot tell it is absent from that
instance. It tries, and treats failure as absence.

What is wrong with that as a contract:

- **Failure stands in for a predicate.** An elaborator defect in an
  available constructor that only the walk reaches is swallowed, and the
  constructor is missing later with no diagnostic.
- **It is sound only because the checker admitted no call.** That is the
  trust P2 removes for method bodies, and a package importer never had it.
- **It covers one member name.** A conditional ordinary method is never
  enqueued eagerly, so it needs nothing today. A conditional copy or move
  constructor is covered only because current Mojo spells both as
  `__init__` overloads. The `__copyinit__`, `__moveinit__`, and `__deinit__`
  symbols are enqueued firm, and no rule says what a conditional one means
  for an instance that does not meet its clause.
- **Demanded and discovered are one bit on a name**, cleared as a side
  effect of `enqueue`.

What the generator must carry instead:

- **The availability clause, on the declaration.** `MirFunctionDeclaration`
  gains the member's `Vec<GenericConstraint>`, lowered from
  `MethodSig.availability` and serialized as binder constraints already
  are. The operands name the owner's and the member's binders by `ParamRef`.
  A clause over value binders uses the same enum's comparison forms. No new
  type is needed.
- **A verdict, not a trial.** Before materializing a member, the elaborator
  evaluates the clause under the instance's `Bindings`. False means the
  member is not in this instance.
- **Reachability as a state.** A request is *demanded* (a call, a witness
  request, or a lifecycle operation in a reachable body) or *discovered*
  (the eager walk). The rules:
  - Discovered and unavailable: skipped.
  - Demanded and unavailable: a compile error at the demanding site.
  - Available, either way: materialized, and any failure is an error.
- **Lifecycle members follow the same rule.** A conditional lifecycle
  member that is unavailable in an instance is absent from it, and a drop
  or copy that needs it is a demand.

With that, `speculative` and the rollback in `run` are deleted. That is
roadmap §1's availability entry.

The conformance question — does `Slot[Int]` conform to `Defaultable` — is
the checker's `traits.rs` today. The elaborator needs the same answer for
concrete types. Conformance tables reach MIR only as the names in
`ParamDecl::Type.bounds`, so the elaborator's evaluator needs each concrete
struct's conformance set, conditional conformances included. That metadata is
listed below, and it is the one piece of P2 that touches `MirStructDeclaration`.

## Pin verdicts

Probes run on 2026-10-01 against Mojo `1.6.0.dev2026092105`.

| Probe | Pin | Mojito | Consequence |
|---|---|---|---|
| `return item^` with `item: Self.T`, `T: Deinitable`, only `Holder[Int]` used | Rejects the declaration: "cannot transfer value into destination, because 'T' doesn't conform to 'Movable'" | Rejects the declaration: "type 'T' … does not conform to trait 'Movable'" (it ran and printed 3 until 2026-10-01) | Row 10 is D (`assets/type_error/template_method_transfer_requires_movable.mojo`) |
| `var other = self.item` with `T: Copyable & Deinitable` | Rejects the declaration: "value of type 'T' cannot be implicitly copied" | Same | Row 3 is D |
| `var item: Self.T` abandoned, `T: Movable` | Rejects the declaration: "'item' abandoned without being explicitly destroyed" | Same | Row 11 is D |
| `def inner() {var item}` capturing `item: Self.T`, `T: Copyable & Deinitable` | Rejects the declaration: "value of type 'T' cannot be implicitly copied" | Rejects the declaration: "capture convention for 'item' requires ImplicitlyCopyable" | Row 26's capability is D |
| `rebind[Int](x)` with `x: T`, called at `String` | Rejects the instance: "function instantiation failed" | Rejects the instance | Row 2 is P |
| `var w: Wrapper[Self.T] = self.item`, `Wrapper` with `@implicit` constructors over `Int` and `Self.T`, at `Box[Int]` | Prints 2: the `Self.T` constructor, bound once | Prints 2 where the template serves the method (`assets/ok/implicit_conversion_bound_on_declaration.mojo`); "ambiguous implicit conversion from 'Int' to 'Wrapper[Int]'" where the method still clones | Row 20 is D. The cloned case is filed (`conformance/probes/implicit_conversion_bound_on_declaration.mojo`) |
| Conditional `__init__` and method over `conforms_to(Self.T, Defaultable)`, one instance with and one without | Runs | Runs, concrete and erased | §Availability describes working behavior |
| Calling the conditional method on the instance without | "invalid call to 'reset': violated constraint" | Same | A demanded unavailable member is an error at the call |

## Package metadata

P6 serializes generators. Its implementation is last, but a generator that
lacks one of these would be reshaped then, so each P2 and P3 change keeps
the list in view.

| Metadata | Today |
|---|---|
| Exported signature: runtime parameters, conventions, result, raise contract | `MirFunctionDeclaration` |
| Binders, bounds, binder constraints | `param_decls`, serialized |
| Member availability clauses | Missing (§Availability) |
| Bodies, drop-elaborated | MIR text |
| Struct layout inputs: fields over binders | `MirStructDeclaration.fields` |
| A struct's conformances, with their conditions | Missing. Checker tables only |
| Trait declarations: requirements and defaults | Missing. A witness request names the requirement's symbol only |
| Transfer and call-through summaries | Missing. Checker tables only |
| Instantiation-time predicates of a body (`rebind`, lane constraints) | Missing. Derivation holds them as `RebindAssertion` |
| Export set: every template, reached or not | The artifact holds one program's whole drop-elaborated MIR. It records no export set |
| Dependency identities of imported packages | Missing |
| Source provenance | File and module ids on locations |
| Schema compatibility | `mir::text::version_header`. A compiler build identity is missing |
| Target assumptions | None in parametric MIR. Layout is asked only of concrete types |
| Cache key: source hash, compiler identity, dependency keys | Missing |

P2 adds two rows: availability clauses, and the conformance set the
elaborator's verdicts need. Both are declaration metadata, and both are in
the package list, so neither is reshaped later.

## What this contract leaves open

- **Compile-time regions.** A `comptime if` arm and a `comptime for` body
  are checked under assumptions the taken arm supplies. How ownership joins
  them is its own roadmap entry, and the guarantee above is stated for
  bodies without them.
- **Value-dependent register types.** Rows 23 and 27 need a type over a
  parameter expression. That is the common type vocabulary entry.
- **Summaries with origin parameters.** Row 14 keeps today's rule, where a
  source vanishes when the substituted type is loan-free. A loan-carrying
  argument is served today by clone origin binders
  (`Bag[Span[Int, __clone_origin0]]`). Without a clone there is no such
  binder, and the generator's summary must name the argument's origin
  positions itself. P2 must settle that for the instances rows 12 and 14
  describe before their clones go.
- **Where the symbolic check is too lenient.** Row 10 closed with P2's first
  step. Row 20 holds for a method its template serves and is still a
  section-3 entry for one that clones. Two more turned up when the clones
  went, each a section-3 entry with a probe: `==` through an `Equatable`
  bound on a type whose own `__eq__` takes another type, and an implicit
  conversion through a consuming constructor.

## What has landed

P2's first step (2026-10-01) serves a generic struct's method from its
template's MIR where the body holds no compile-time construct and the
instance's arguments are plain data.

| Rows | State |
|---|---|
| 1, 4, 5, 8 | Done for those methods: the elaborator binds the owner's parameters from the receiver, or from `Call.receiver` for a static call, and the driver reads the instances a template body reaches off its checked types (`src/compiler/template_reach.rs`). |
| 3, 10, 11 | Proved on the declaration. Row 10 is new. |
| 14 | A call site keeps a source only where a loan can ride it. A symbolic value read out of the frame's own storage lends that storage latently. An instance over a loan-carrying argument still clones (roadmap §1). |
| 15 | The elaborator selects the dunder. A `!=` with no `__ne__` negates `__eq__`, and a sized-scalar comparison the template typed `Bool` converts its mask. |
| 18 | The template's selected constructor serves the instance, a variadic one keyed by the call's element count. |
| 7, 16, 18 (availability) | Still decided by the checker at the call. The clause is not in MIR (roadmap §1). |
| 2, 23, 24, 26, 27 | Unchanged: these bodies hold a compile-time construct and keep their clones until P3. |
