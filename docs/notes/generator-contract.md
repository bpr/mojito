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
| Availability of a conditional member | `GenericConstraint` (the checker's `MethodSig.availability`) | `MirFunctionDeclaration.availability`. See §Availability |
| A struct's conformances, with their conditions | `StructConformance`: a trait and the `GenericConstraint`s any of which proves it | `MirStructDeclaration.conformances`, and the declared traits in `MirDeclarations.traits` |
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

## Availability

A member-level `where` clause is the checker's `MethodSig.availability`. It
reaches MIR on the member's declaration, and the elaborator decides it under
an instance's `Bindings` (`mono/availability.rs`).

What the generator carries:

- **The availability clause, on the declaration.**
  `MirFunctionDeclaration.availability` holds the member's
  `Vec<GenericConstraint>`, one entry per clause, serialized as binder
  constraints are. The operands name the owner's and the member's binders by
  `ParamRef`. No new type was needed.
- **The conformance rows, on the struct.** `MirStructDeclaration.conformances`
  holds one row per trait that an availability clause or a conformance
  condition of the program names. A row lists conditions over the struct's
  own binders, and an instance conforms when its arguments satisfy any of
  them. No condition means no instance conforms. The checker builds the rows
  from the same rules `conforms_to` applies to a struct
  (`Checker::conformance_facts`), so the elaborator ranks nothing and
  re-derives nothing.
- **The declared traits, on the program.** `MirDeclarations.traits` lists
  them. A compiler-known type conforms to none of them, and to a built-in
  trait by its shape alone. That shape rule is one function,
  `mojito_types::conformance::leaf_conforms`, which the checker's
  `conforms_to` and the elaborator both call.

How the elaborator uses it:

- **A verdict, not a trial.** Before materializing a member, the elaborator
  evaluates the clause. The verdict is proven, disproven, or undecided.
- **Reachability is the caller's.** A request is *demanded* (`enqueue`: a
  call, a witness request, or a lifecycle operation in a reachable body) or
  *discovered* (the eager walk in `discover_structs`).

  | Request | Proven | Disproven | Undecided |
  |---|---|---|---|
  | Discovered `__init__` | Materialized | Skipped | Skipped. A call site demands it |
  | Discovered `__copyinit__`, `__moveinit__`, `__deinit__` | Materialized | Skipped | Materialized. Lowering composes the name |
  | Demanded | Materialized | A compile error naming the member, the instance, and the clause's message | Materialized on the checker's word |

- **A failure is an error.** Nothing is rolled back. A member that is
  available and does not materialize fails the compilation.
- **An instance keeps nothing to decide.** An elaborated declaration has an
  empty `availability`, and an instance struct's rows hold `Bool(true)` or
  nothing. `mir::verify::verify_concrete` rejects anything else.

What is undecided: a clause over `IsTrivially*`, a pack predicate, or a value
expression, and a question asked of a struct that answers by its elements
before its specialization exists. That is roadmap §1's entry on undecided
clauses. The contract's rule that concrete bindings close every predicate
(§Predicates) holds once that entry lands.

Removing the trial exposed one defect it had hidden. `Array[c_char, n]()`
never materialized, because `T()` over a SIMD binding had no construction.
The eager walk reached that constructor in a program that never called it,
and the rollback dropped it. The elaborator now builds the zero vector.

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
| Conditional `__init__` and method over `conforms_to(Self.T, Defaultable)`, one instance with and one without | Runs | Runs, concrete and erased | §Availability describes working behavior (`assets/ok/conditional_member_availability.mojo`) |
| Calling the conditional method on the instance without | "invalid call to 'reset': violated constraint" | Same | A demanded unavailable member is an error at the call |

## Package metadata

P6 serializes generators. Its implementation is last, but a generator that
lacks one of these would be reshaped then, so each P2 and P3 change keeps
the list in view.

| Metadata | Today |
|---|---|
| Exported signature: runtime parameters, conventions, result, raise contract | `MirFunctionDeclaration` |
| Binders, bounds, binder constraints | `param_decls`, serialized |
| Member availability clauses | `MirFunctionDeclaration.availability`, serialized |
| Bodies, drop-elaborated | MIR text |
| Struct layout inputs: fields over binders | `MirStructDeclaration.fields` |
| A struct's conformances, with their conditions | `MirStructDeclaration.conformances`, for the traits a clause or a condition names |
| Trait declarations: requirements and defaults | Names only (`MirDeclarations.traits`). A witness request names the requirement's symbol only |
| Transfer and call-through summaries | Missing. Checker tables only |
| Instantiation-time predicates of a body (`rebind`, lane constraints) | Missing. Derivation holds them as `RebindAssertion` |
| Export set: every template, reached or not | The artifact holds one program's whole drop-elaborated MIR. It records no export set |
| Dependency identities of imported packages | Missing |
| Source provenance | File and module ids on locations |
| Schema compatibility | `mir::text::version_header`. A compiler build identity is missing |
| Target assumptions | None in parametric MIR. Layout is asked only of concrete types |
| Cache key: source hash, compiler identity, dependency keys | Missing |

P2 added two rows: availability clauses, and the conformance rows the
elaborator's verdicts need. Both are declaration metadata, and both are in
the package list, so neither is reshaped later.

## What this contract leaves open

- **Compile-time regions.** A `comptime if` arm and a `comptime for` body
  are checked under assumptions the taken arm supplies. How ownership joins
  them is its own roadmap entry, and the guarantee above is stated for
  bodies without them.
- **Value-dependent register types.** Rows 23 and 27 need a type over a
  parameter expression. That is the common type vocabulary entry.
- **A `def` the elaborator cannot instantiate.** A plain trait-bound
  generic `def` is served by its template at every closed call (§Carried
  sources for the loan-carrying ones). It still clones, with clone origin
  binders at a loan-carrying argument, where it carries a value parameter
  or where its body holds or reaches a compile-time construct. A clone over
  an origin binder that stores a copy records no loan. Each is a roadmap
  entry.
- **Where the symbolic check is too lenient.** Row 10 closed with P2's first
  step. Row 20 holds for a method its template serves and is still a
  section-3 entry for one that clones. Two more turned up when the clones
  went, each a section-3 entry with a probe: `==` through an `Equatable`
  bound on a type whose own `__eq__` takes another type, and an implicit
  conversion through a consuming constructor.

## Carried sources

A template's store of a value of a parameter type names no loan: the loans
are the instance's. Its transfer summary therefore carries the stored type.

- **The effect.** `TransferEffect.src` is `SigOrigin::Carried(type)`, with
  the destination the store wrote (`self.items`, a `mut` parameter). It
  stands for every origin the type names once the callable's binders are
  bound. The body records it where it accepts an outward store of a symbolic
  value (`check_outward_store`), and where it hands a symbolic value by
  value to a callee that stores it.
- **The call.** `replay_transfer_effects` closes the type with the
  arguments of the struct instance the receiver is, and with the types the
  call binds to the callee's own binders: a generic `def`'s, or a method's
  own. A type that names a binder the call leaves unbound is skipped. The
  places the closed
  type names (a reference's or a pointer's origin, a struct's origin
  arguments) are the sources, and the destination borrows them shared. A
  type still symbolic at the call is carried on in the caller's own summary.
  A plain-data type names no place and records nothing.
- **No binder.** Nothing in the summary names an origin parameter, so no
  clone origin binder is needed to spell it. A plain trait-bound `def`
  keeps its template at every closed call, loan-carrying or not, and a
  loan-carrying instance is served by its template as a plain-data one is.
- **The escape verdict** is the call's: a carried source rooted in the
  caller's frame cannot reach a destination that outlives it.
- **A nested `def`** records none. Its summary key is its bare name in every
  copy of the enclosing body, and that body is cloned per instance.
- **Serialized.** `sig_carried(type)`, text schema 1.10.

A derived clone drops a carried source its instance closes, and respells one
still symbolic, as its own check records it.

## What has landed

P2's first step (2026-10-01) serves a generic struct's method from its
template's MIR where the body holds no compile-time construct and the
instance's arguments are plain data. Since 2026-10-02 the instance's
arguments may carry a loan too (§Carried sources).

| Rows | State |
|---|---|
| 1, 4, 5, 8 | Done for those methods: the elaborator binds the owner's parameters from the receiver, or from `Call.receiver` for a static call, and the driver reads the instances a template body reaches off its checked types (`src/compiler/template_reach.rs`). |
| 3, 10, 11 | Proved on the declaration. Row 10 is new. |
| 14 | A call site keeps a source only where a loan can ride it. A symbolic value read out of the frame's own storage lends that storage latently. A store of a symbolic value publishes its stored type as a carried source, and a call through a struct receiver closes it (§Carried sources). An instance over a loan-carrying argument clones no more than a plain-data one. |
| 15 | The elaborator selects the dunder. A `!=` with no `__ne__` negates `__eq__`, and a sized-scalar comparison the template typed `Bool` converts its mask. |
| 18 | The template's selected constructor serves the instance, a variadic one keyed by the call's element count. |
| 7, 16, 18 (availability) | The clause is in MIR and the elaborator decides it (§Availability). A clause in a form it cannot evaluate is undecided, and the checker's verdict at the call stands (roadmap §1). |
| 2, 23, 24, 26, 27 | Unchanged: these bodies hold a compile-time construct and keep their clones until P3. |
