# Deriving instantiations from checked templates

Mojo checks a parametric body once, with its parameters symbolic, and
instantiation substitutes into the checked result. Mojito used to do the
reverse: the elaborator cloned a body per instantiation and the checker
inferred every clone from scratch, once per discovery round and once per
transfer-effect round. This note records the mechanism that makes a checked
template the authority for the instantiations it covers, what it covers
today, and what it does not.

The plan this implements is `instantiation-from-template-plan.md` (untracked,
repository root). The remaining work is `docs/roadmap.md` section 1, one
entry per uncovered construct.

## The carrier stays the AST clone

HIR and MIR still walk a concrete AST plus the checked tables. Nothing below
`CheckedProgram` changed. What changed is where a covered clone's facts come
from: the checker installs facts derived from the template instead of
inferring the clone's body. A template is never executable and never reaches
HIR or MIR.

## Code anchors

| Piece | Owner |
|---|---|
| Template vocabulary: `TemplateId`, `CheckedTemplate`, `CheckedBodyFacts`, `TemplateCoverage`, `TemplateClass`, `TemplateObligation`, `TemplateCatalog`, `InstanceTrace`, `OccurrenceId`, `FactTable` | `crates/mojito-checked/src/templates.rs` |
| Exhaustive `SemanticAdjustment` policy | `templates.rs:derive_adjustment` |
| Capture, certificate, realization, installation, verification | `crates/mojito-checker/src/checker/template_facts.rs` |
| Re-selection of a call through a bound (`bound_witness`, `realize_bound_dispatch`, `realize_inverted_writes`, `realize_bound_builtin`) | `crates/mojito-checker/src/checker/template_facts/bound_dispatch.rs` |
| Body entry points | `template_facts.rs:Checker::check_def_body` (from `statements.rs:check_def_inner`) and `check_method_body` (from `declarations.rs:bind_and_check_method`), both over one `BodySite` |
| Occurrence-level trace | `crates/mojito-ast/src/ast.rs:rekey_syntax` returning `SyntaxOrigins` |
| Declaration-level trace | `crates/mojito-comptime/src/comptime.rs:DefInstanceTrace` (`specialize.rs:generate_def_spec`) and `MethodInstanceTrace` (`generate_instance_clones`) |
| What the elaborator generated | `comptime.rs:GeneratedDeclarations`, carried as `templates.rs:GeneratedNames` |
| Statement identity through elaboration | `comptime.rs:rebuilt` |
| Catalog lifetime, trace hand-over, one finalization | `src/compiler.rs:compile_linked`, `instance_traces` |
| Discovery without the arena | `crates/mojito-checked/src/checked.rs:DiscoveryResult`, `checker.rs:check_program_for_discovery` |

## What a checked template is

A template is a module-level generic `def` or a method of a generic struct. A
`Method` has no source range of its own, so a method template is identified by
its module, its struct, its name, and the range of its body's first statement
(`TemplateId::owner`); two overloads of one name are two templates.

A `CheckedTemplate` is one generic declaration's parameter declarations, the
facts its single body inference recorded, a coverage certificate, and the
obligations every instance still owes. The facts are keyed by the body's own
syntax occurrences, never by a checker run's spans, and bindings are named in
template-local terms (`TemplateOwner`): a runtime parameter by index, a
method's `self`, a local by declaration order, a module-scope declaration by
name. Checker owner identities are per-run counters and mean nothing in
another run.

There is one producer per body:

- **The executable check** produces the template of a trait-bound generic that
  survives elaboration as an erased body.
- **Source validation** produces the template of a body it checks and the
  elaborator then stubs: one keyed by compile-time control flow or by a
  `rebind`. `validate_comptime_templates_into` lends the catalog to that run.

The catalog lives for one `Compiler::compile_linked`. It buckets templates by
`TemplateId` and compares exactly. A mangled symbol erases origins and `Ty` is
not uniformly hashable, so neither is a key.

## Capture is total, or it refuses

`FactTable` enumerates every occurrence-keyed table the checker fills, and
`span_table` maps each onto the checker's storage. Capture accounts for all of
them:

- A table with a derivation recipe carries its entries across.
- An entry in a table without a recipe refuses the body
  (`IncompleteReason::UnsupportedTable`).
- A table that grew by more than the body's own occurrences explain refuses it
  (`FactOutsideBody`): some fact was keyed by synthesized syntax.
- Growth in a store that is not keyed by occurrence refuses it
  (`UnkeyedFact`): hash leaf types, nested declaration types and effects,
  transferred origins, deletable declarations.
- A callee effect summary that was read and was not empty refuses it.

Two things a body reaches are not facts at an occurrence, so capture reads
them from a per-body frame instead of from table growth. The frames exist only
for a body that will be captured.

- **Effect summaries read** (`effect_query_frames`). The observation map is
  first-seen per check, so its growth would miss the second reader of a callee.
- **Generic-struct applications reached** (`struct_application_frames`,
  pushed in `generics.rs:record_struct_instantiation` before its filters). A
  template records `List[T]`, which discovery ignores as open, where its clone
  records `List[Int]`, which mints that instance. A derived clone therefore
  substitutes the template's applications and records them itself, under its
  own source. They are kept as a set: recording is idempotent, and a clone
  check reaches a retargeted receiver's application twice.

Capturing walks every fact table for every occurrence, so the syntax is judged
first (`certificate(site, None)`): a generic declaration outside every class is
retained as such without being captured.

A new fact table must be added to `FactTable`, which forces a policy decision
in `span_table`, `derivable_table`, and `remove_occurrence_facts`.
`replace_body_facts` checks the last list against `FactTable::ALL`. A new
`SemanticAdjustment` must be given a policy in `derive_adjustment`, whose match
has no wildcard.

Tables with a recipe today: expression types, place types, binding types,
expression and statement bindings, expression effects, operation adjustments
(`MaterializeLiteral`, `PointerOffset`, `PointerStorageTake`,
`PointerStorageDestroy`, a moving `PointerWrite`, and the inverted writes,
which an instance re-selects), generic instantiations, method instantiations
(a per-call request whose arguments stay symbolic under the instance),
overload targets, call parameters, selected calls (a `value_method_contract`,
a `closed_reference_contract`, or the abstract dispatch of a bound),
borrowed read call places, read temporary arguments, unconsumed temporaries,
discarded results, deletable and linear bindings, linear temporaries, copied
places, interior invalidations, rebind assertions, reference handles
(`ReferenceValueUses`, at the value of a `return` in a method that returns a
reference, and nowhere else), reference results (the `ReferenceResult`
adjustment, kept apart from the other adjustments), interior references, and
copyable reference-result reads.

Three things are recorded and kept as a fact about the body rather than as
entries. A call transfer (`CallTransfers`), the origins it merged, and the
effect the body's own frame then publishes exist only while a value may carry
a loan, so the bundle keeps one flag (`vanishing_transfers`) and an instance
owes that none of its values can. A comparison of two parameter-typed places
records nothing in the template, so the grammar names it (`comparisons`) and
an instance dispatches it itself. A checker builtin on a bounded parameter
(`hasher.update(x)`, `writer.write(x)`) selects no callee and records nothing
that names the call, so the grammar names it too (`bound_builtins`) and an
instance proves the argument's bound again.

Two things are kept in template-local form because they belong to one checker
run. A binding identity becomes a `TemplateOwner`. A selected call becomes a
`TemplateCallContract`: the contract with its boundary emptied, each supplied
argument named by its occurrence rather than its span, and each invalidated
place by template owner. Capture also refuses a body whose retained types name
a place (an origin rooted at a binding identity, in a pointer, a reference, or
a struct's origin argument), since a type is kept as written and nothing would
remap the identity inside it.

A reference a call yields is the one place a binding identity sits inside a
type, and it is kept as a `TemplateReference`: the referent, the mutability,
and a `TemplateOrigin` whose places are rooted at a template owner
(`TemplatePlace`). It appears three times for one call — the
`ReferenceResult` adjustment, the contract's `reference_result`, and the
contract's result type — and once more, as a bare place, in
`InteriorReferences`. Capture moves all four into template-local form (the
stored contract's result type becomes the referent) and installation rebuilds
them on the instance's own receiver. Only a receiver or a parameter may root
one. The recorded expression types of such a body are the referents, because
ordinary inference reads through a reference, so the guard on the three type
tables stands unchanged.

## The expansion trace

Two halves identify what a clone's occurrence came from. Neither demangles a
symbol.

- **Declaration level.** `generate_def_spec` records a `DefInstanceTrace` per
  clone: the prepared declaration it instantiates, the source type written for
  each baked type parameter, each folded value parameter, and the parameters
  the clone still declares. The driver converts these to `InstanceTrace` and
  hands them to the catalog before each check.
- **Declaration level, methods.** A per-instantiation clone is appended to its
  template struct's own method list (`get$y3:Int` on `Box`, with
  `Method::self_ty = Some(Box[Int])`). `generate_instance_clones` records a
  `MethodInstanceTrace`, and the clone is named by its struct, its name, its
  body's source tag, and the range of its body's first statement
  (`InstanceName`): same-name overloads clone under one name and one tag. A
  per-call clone, which also bakes the method's own parameters, leaves no
  trace yet.
- **What was generated.** `GeneratedDeclarations` lists every `def` clone,
  every struct specialized whole (`Tuple$…`), and every per-call method clone.
  That list, or an explicit receiver type, is the only test for "generated": a
  `$` in a name proves nothing, since a module-qualified source name
  (`__module$std$string$String`) carries one too.
- **Occurrence level.** A clone node keeps the syntax identity of the template
  node it was copied from. `rekey_syntax` makes identities unique for the
  final tree and now returns `SyntaxOrigins`, the identity each re-keyed node
  had before. The elaborator's `rebuilt` keeps a statement's identity when it
  rebuilds that statement with new contents.

An `OccurrenceId` is that pre-rekey identity plus a copy number. A template's
occurrences are all copy zero. An unrolled `comptime for` copies one template
occurrence once per iteration, and the copies are numbered in pre-order.
`CheckedBodyFacts::selected` lays a template's facts out over an instance's
occurrences: a dropped occurrence (an untaken arm, a zero-trip loop) takes its
facts, requests, and effect reads with it, and a copied one carries them once
per copy.

An instance occurrence with no template occurrence behind it refuses the
derivation. A folded value parameter is such a case today: the elaborator
writes a fresh literal where the identifier stood.

## Certificate classes

Every class shares a declaration shape: a module-level `def`, plain type
parameters (plus scalar `Bool`/`Int` value parameters for a keyed body),
immutable regular runtime parameters, a concrete scalar result, and no
`raises`, captures, or decorators. An operator is admitted only over operands
whose recorded types are closed scalars, so no operator dispatches through a
bound. `BodyShape` is the grammar; `template_certificate` is the argument.

| Class | Body | Why an instance needs no inference |
|---|---|---|
| `ClosedScalarBody` | `return`s of closed scalar expressions | The body names nothing, so every fact is closed and inherited unchanged. |
| `FixedCalls` | plus direct calls of module-scope functions, passing literals, parameters, and further such calls | Selection is retained. No conversion, copy, or adjustment was recorded, so every argument matched its parameter exactly and still does after substitution. Borrows depend on slots and conventions, not types. The callee's effect summaries were empty and are re-read. |
| `BoundedOperations` | plus the built-in `len` over a parameter whose bound promises a length | The bound proved the call. The instance owes the witness, which `len_result_for_type` finds, and takes the read-in-place fact `infer_len` adds for a nominal struct. |
| `MethodScalarBody` | a method with a plain read `self` and no binders of its own, on a struct of plain type parameters: `return`s over closed scalars, parameters, reads of `self`'s scalar fields, the built-in `len` over a field, and argument-free method calls on `self` or a field with a trivial contract | A field read has the field's declared type under the struct's arguments in a template and a clone alike. The generated-declaration leniency a clone's name switches on bears on origin-bearing return annotations only, and the result is a scalar. A trivial call can change per instance only in its target. |
| `ScalarBranches` | source-validated bodies: `comptime if` arms, scalar `comptime for` loops, scalar locals and assignments, erased `rebind`s | Every arm was checked once. The instance keeps the occurrences the elaborator selected. |
| `MethodBody(features)` | a method beyond `MethodScalarBody`; see below | One argument per feature. |

A body source validation did not produce (every class but `ScalarBranches`)
may also hold runtime statements over closed scalars: scalar locals and
assignments, `if`/`elif`/`else`, `while`, `break`, `continue`, a bare
`return`, and a discarded call or `_ =` value. A runtime statement is checked
once whatever runs it, so none drops or copies an occurrence. A condition's
recorded type must be exactly `Bool`, which `expect_bool` accepts without a
truthiness fact. A keyed body keeps its own rules: a local is never declared
inside a `comptime for`, where an unrolled body would need one binding per
copy, and the loop variable may only key a condition.

### `MethodBody`

The declaration may have a `mut`, `var`, `deinit`, or `ref` receiver, the
`out` of an `__init__`, or none (`@staticmethod`), a `where` clause, `var`,
`mut`, and bare `ref` parameters, trait-bounded type binders of its own, and
any result, a reference included. A
`mut` or `ref` parameter is bound from its declared convention and rooted at
its own binding under every instance, so the body's facts name it by template
owner; what its caller owes lives in the signature, which is checked per
clone. The body may store to a `mut` parameter, a scalar or a whole value of
the parameter's own type, and may only read a bare `ref` one, which it never
moves out of. Which fields an `__init__` initializes is
its syntax, and definite initialization is judged outside the body check. A
bare `ref self` is a receiver like the others: it has parametric mutability, so
the body cannot write through it, and one binding identity names it under
every instance. A `ref` parameter may carry an origin clause, and the method
may declare the origin binders such a clause names (`[o: Origin]`,
`ImmOrigin`): a binder is inferred at every call and erased before execution,
no clone is minted per origin, and a clone keeps the binder and binds it
symbolically as the template does. A body that writes through a `MutOrigin`
is judged per instantiation and stays a clone check. A receiver origin, a
struct's own origin parameter (no clone is minted for such a struct), the
copy and move initializers, any other binder, and `raises` stay outside. A `where`
clause is the declaration's constraint: `generate_instance_clones` mints a
clone only where it evaluates true, a trace exists only for a minted clone, and
a clone's signature no longer states it.

`MethodFeatures` names what the body holds. The features are independent, so
they are a set, not a ladder.

| Feature | Body | Why an instance needs no inference |
|---|---|---|
| `STATEMENTS` | a receiver other than a plain read `self`, or the runtime statements above, plus a store to a closed scalar field of a `mut`/`var` `self` | A stored field is a closed scalar, so the store is a plain scalar write and never an in-place operator of the field's type. Invalidations name `self` and locals by template owner. |
| `OPAQUE_MOVES` | a whole value of any type moved (`^`) or copied between a parameter, a local, a field of `self`, and the result | The value is never an operand, a receiver, a condition, or an argument, so nothing dispatches on its type. A store or a result has the value's own recorded type, which stays equal under substitution, so neither check converts. What a clone check still decides from the type is owed per instance (obligations 3 and 10 to 12 below). |
| `POINTER_SLOTS` | over a pointer field of `self` with no tracked provenance: `unsafe_offset(scalar)`, `unsafe_take_pointee()`, `unsafe_deinit_pointee()`, `free`, and the slot `pointer[scalar]` as a store target or a copied place | Such a pointer holds no loan, names no place, and is a pointer under every instance, so its methods are the built-in ones (`infer_pointer_method`), which select no callee and record an adjustment naming at most the pointee. Every other judgment there only produces an error, and the template's is at least as strict. |
| `SIBLING_CALLS` | a method call on `self` or a field of it, passing closed scalars, whose contract is a `closed_method_contract` | Obligation 7 below. Arguments are closed scalars in both checks, so nothing about them depends on the instance. |
| `REFERENCE_RESULT` | a method that returns a reference: every `return` hands out a field of `self`, a pointer slot, or a reference call's result, of exactly the declared referent type | The `return` keeps its value as a handle because the declaration returns a reference (`ReferenceValueUses`, written from the declaration and the statement's syntax alone) and demands neither a copy nor a move. Whether the place lies within the declared origin is judged on its path, the signature, and the receiver's constructor, none of which an instance changes; the loan-escape check reads what obligation 12 already rules out. The signature is checked per clone before the body, derived or not. Any other handle in the body refuses it. |
| `REFERENCE_CALLS` | a subscript or a named accessor on a field of `self`, passing closed scalars, whose contract is a `closed_reference_contract`, either returned as above or read by value into the result or an unannotated `var` | Obligation 13 below. The call is never a receiver, an operand, a condition, or an argument, each of which records a borrow of its own. A by-value read is admitted only where the template marked the read copyable. An iterator's `__next__` marks its read by another rule and stays out. |
| `REFERENCE_LOCALS` | `ref name = place`, where the place is `self`, a field of it, a parameter, a `var` local, or a reference call; then a field read through the binding, the binding copied out whole, a scalar operand, `len` over it, a scalar store through it, or the binding forwarded as the method's own reference result | The declaration decides mutability from the value's reference and the binding it names, re-stamps an origin computed upstream, and runs no check of its own. Its type is a reference whose origin names a binding, so a bundle keeps it by template owner, as it keeps a call's, and an instance substitutes the referent. Every use records the referent. A copy out owes obligation 3, and a copyable read obligation 13. |
| `REFERENCE_RECEIVERS` | a field read or a closed method call through a reference call's result or a `ref` local whose referent is a struct | A call borrows such a receiver (`BorrowedReferenceReceivers`) because of what the receiver is: a reference result, a `ref` binding, a `ref` field. None of those tests reads the referent. The callee is realized as in obligation 7, from the receiver's own substituted type; a receiver whose type was already closed in the template (`List[Pair]`) selected its clone there. A method of a bare parameter dispatches through the bound and stays out. |
| `SUBSCRIPT_STORES` | a store through a subscript of a field of a writable `self`: a closed scalar field of the element a reference getter yields (`self.entries[i].hits = 0`, `+= 1`), or a closed scalar element a declared setter takes (`self.counts[i] = n`) | A subscript that is a place records its index shape and whether the setter takes the value by keyword (`SubscriptDescriptors`). The first is the syntax, one plain index; the second is the setter's declaration. The getter is a `REFERENCE_CALLS` call and the setter a closed `SIBLING_CALLS` one, recorded at the subscript and realized by the setter's name. A whole element of a parameter type stays out: `self.items[i] = self.items[j]` is accepted for `Int` and refused for `String`. An augmented element store (`self.counts[i] += 1`) records `AugmentedSubscript`, which has no recipe. |
| `PLACE_ARGUMENTS` | a local, a parameter, or a field of `self` handed to a `mut` or bare `ref` parameter of a method call | The callee's declared convention decides that the call keeps the caller's place (`CallPlaceUses`), and the generations a `mut` argument invalidates lie below the argument's own binding, kept by template owner. A kept place is neither copied, moved, nor converted, and its recorded type equals the parameter's. Whether two arguments conflict is judged on their places and conventions. A field of `self` is kept only beside a receiver the call reads. A parameter of a struct parameter's type is admitted only on a call of `self`'s own method, where callee and caller share one binder scope, so the instance substitutes it in the contract and in `CallParameters`. |
| `ORIGIN_PARAMETERS` | a `ref` parameter with an origin clause, the method's origin binders, and the parameter forwarded as the method's own reference result | The clause lives in the signature, which is checked per clone. The body's facts for such a parameter are a bare `ref` one's: its binding, its type, and a copy where it is read by value. |
| `VALUE_ARGUMENTS` | a whole value of any type handed to a by-value parameter of a method call: a `^` transfer, a sibling call's result, a place the template copied, or a named place a read parameter takes where it lies | The argument's recorded type equals the parameter's, so nothing converts it, before or after substitution. What the call records for it is decided without its type: a read parameter borrows a named place and reads a temporary by the argument's syntax and the callee's conventions, and a `var` parameter takes a transfer or a temporary as it stands. A copied place owes obligation 3 and a transfer obligation 10, as elsewhere. A `ref` local and a reference call's result stay out, since each records a borrow of its own. A callee with a parameter of a struct parameter's type may belong to a field of another struct: it has no binders of its own, so its parameter types were recorded at the receiver's arguments, in the caller's binder scope, and the instance substitutes them in the contract and in `CallParameters` alike. An overloaded family whose members declare parameters of parameter types is admitted when every argument's type is exactly its parameter's: no member outranks an exact match, and a family an instance collapses finds no single clone (`method_clone_target`). |
| `VANISHING_TRANSFERS` | a call whose callee stores an argument outward (`self.items.append(value^)`), so the template replays a transfer summary at it | A transfer moves the loans its source carries. The template's parameter may carry one (`type_may_carry_loans` is true for a symbolic parameter), so its check records a call transfer, merges the origin, and publishes an effect of its own; every value of a plain-data instance carries none, so `replay_transfer_effects` records nothing and the instance's frame publishes nothing, which is what the clone check records. Obligation 14 below. A call-through residue, a function value's baked effects, and a destination a captured binding names are not transfers and still refuse. |
| `OPERATOR_DISPATCH` | `==`, `!=`, `<`, `<=`, `>`, `>=` over two places of one type that mentions a struct parameter | The template proves the operator through the bound and records nothing at it; `infer_infix` decides a struct operand's dunder from the operand types alone (`struct_infix_dispatch`). Obligation 15 below. Each operand is a place, so both checks read it where it lies and no borrow or copy is recorded. |
| `BOUND_DISPATCH` | a method call on a place of a bare parameter type, proved through a bound: `place.copy()`, `item.__hash__(hasher)`, `item.write_to(writer)`, or any requirement whose arguments are closed scalars or named places of a bare parameter type handed to a bounded `mut`/`ref` parameter | The template records the abstract dispatch (`__trait_dispatch.__hash__$ov$…`), or for `write_to` the inverted write, and nothing the receiver's type decides: a kept argument's place use and generation refresh follow from the requirement's convention, which the witness shares. `infer_method_call` decides a concrete receiver's witness from its type alone, and `bound_witness` repeats that decision from the types. Obligation 16 below. The call through the bound read the summaries of every conformer's method of that name, one key per conformer; each was empty, and the instance re-reads only its own target's. |
| `BOUND_BUILTINS` | `hasher.update(x)`, `hasher._update_with_simd(x)`, or `writer.write(x…)` on a parameter bounded by `Hasher` or `Writer`, whose arguments are closed scalars, string literals, named whole values, `ref` locals, fields read through a reference, pointer slots, or reference calls | The builtin selects no callee and records at an argument only what its syntax decides: a borrow of a named place or a reference result, an unconsumed temporary, a closed literal's materialization. The argument's type it proved through the bound, and the instance proves it again at its own type (obligation 17), which for a hashed value is where the hash leaf is recorded. |
| `BOUND_BINDERS` | the method's own type binders, each a plain trait-bounded type (`[H: Hasher]`) | A clone keeps such a binder, bound symbolically as the template binds it: no clone is minted per hasher, the VM reifies the binding at run time, and no retained fact substitutes it. The instance's substitution binds the struct's parameters alone (`instance_substitution`), as it does beside an origin binder. |
| `CONSTRUCTIONS` | a construction of a declared struct whose compile-time arguments are types, as a whole value: `copy:` of a named place, or arguments that are closed scalars, whole values, or — for a fieldwise struct's reference field — a `ref` local | A construction records no contract: `infer_construction` selects an `__init__` from the argument types, retargets it to the instance's constructor clone, and reaches the struct application. Its one unconditional record, `ConstructionImmutableBinders`, is empty when no compile-time argument is an `ImmOrigin` cast, which the type-only arguments guarantee, and installation writes the empty entry again. What a constructor records at an argument is decided by the argument's syntax and the constructor's conventions (a copy owes obligation 3, a transfer obligation 10, a literal materializes to a closed type); the selected member binds each argument exactly, so no member can outrank it under any instance (obligation 18). A constructed type that names the receiver in an origin argument (`_ListIter[T, origin_of(self)]`) is kept with the slot unbound and the origin by template owner (`typed_origins`), and the `return`'s re-resolution of the annotation's `origin_of(self)` is repeated once by the instance rather than recorded. |

The compiler-private trap `_mojito_abort("message")` is a statement of any
non-keyed body: the built-in types its literal and selects nothing. A
declaration of that name would record a binding and parameters at the call,
and such a call is not admitted.

A bare place of a parameter type is admitted at a consuming position only
where the template recorded the copy (`copy_place_value_uses`). A type that is
only `Movable` records nothing there, and its `Int` clone would.

## What an instance still owes

`realize_instance_facts` substitutes and then discharges, in this order:

1. **Substitution.** For a `def` clone, each baked type parameter stands for
   the source type the elaborator wrote, resolved as the clone's own
   annotations are. For a method clone it is the arguments of the receiver
   type the checker already resolved for it: the raw request passes through
   origin erasure and literal defaulting before a clone is minted, so only the
   resolved receiver says what `Self.T` is inside it
   (`instance_substitution`). A callee's declared parameter types are in the
   callee's binder scope and are never substituted (`CallParameterFact`).
2. **`rebind` equalities.** Source validation takes `Dest` on faith. The
   template keeps the operand's own type, the target, and the by-value
   selection (`RebindAssertion`), and the instance owes their equality.
3. **Implicit copies.** A place copied at a consuming position must be
   implicitly copyable at the instance's type, as `check_consuming` demands.
4. **Locals.** The survivors are renumbered as the instance's own check would
   mint them.
5. **Direct calls.** The template's selection stands. The instance repeats the
   one concrete decision a clone check also makes after selection, whether the
   closed application already has a clone (`existing_def_clone`). When the
   elaborator already retargeted the call, the written name must be exactly
   that clone, and the call takes the clone's own declared parameters.
6. **Built-in `len`.** See `BoundedOperations` above. A borrow the template
   already recorded for the operand (a reference-valued one) is kept; only the
   nominal-place rule can newly hold for an instance.
7. **Closed method calls.** `closed_method_contract` names every field of
   `CheckedCallContract`: a receiver read or mutated in place, every argument
   supplied and either bound by value to a closed scalar parameter, adapted at
   most by materializing a literal to a closed type, or kept as the caller's
   place for a `mut` or `ref` parameter (`kept_place_argument`), and no raise,
   result adapter, reference result, captures, or compile-time parameters. A
   `value_method_contract` is the same but for a by-value parameter of any
   type, whose argument then takes no adjustment at all (`VALUE_ARGUMENTS`). A
   `trivial_method_contract` is the case with no arguments, a plain read
   receiver, and a closed result, which is all `MethodScalarBody` admits.
   `realize_method_call` repeats the clone check's retarget: the declared
   member is the one whose lowered name the template recorded, and the clone
   member is the one with that signature (`method_clone_target`, shared with
   `constructor_clone_target`), giving `List.pop$y3:Int$ov$Int`. A clone check
   ranks the clone family again on its arguments, so every member of an
   overloaded family must declare closed parameter types for the two rankings
   to agree, unless every argument's type is exactly its parameter's, which
   no member outranks. The result type substitutes. A method call's parameter
   types are in the receiver's binder scope, whether the receiver is `self`
   or a field of another struct, unlike a direct call's `CallParameterFact`,
   so the contract's and the `CallParameters` entry's substitute alike. A
   clone that exists has met its `where` clauses; a callee with an
   availability condition and no clone refuses, as does an instance that has
   clones but not this one (withheld, or a collapsed overload family).
8. **Struct applications.** Substituted, then recorded by installation under
   the body's own source.
9. **Effect summaries.** Every callee's transfer and call-through summaries
   must still be empty. Installation records the same empty observation a
   clone check would, so the transfer fixpoint re-runs if one grows.
10. **`Movable`.** Each `^` transfer of a value whose type mentioned a
    parameter must be of a `Movable` type for the instance. A parameter is
    always movable while it is symbolic and the demand only ever produces an
    error, so no retained fact carries it and verification cannot see it
    (`assets/type_error/template_method_transfer_requires_movable.mojo`).
11. **Deletability.** A binding of a bare parameter type is deletable, linear,
    or neither at the instance's own type, as the declaration's check decides
    it. A binding whose type is built over a parameter (`Optional[T]`) leaves
    no entry to judge again and refuses. A linear temporary stays one only
    while its type is still a parameter, which an instance's never is.
12. **Plain-data arguments** (`MethodBody` only). Every instance argument
    carries no loan, holds no reference, and mentions no callable. A clone
    check decides outward-store transfer effects, view-result borrows, and
    closure escapes on those properties, and a template, whose parameter is
    symbolic, records none of them. `type_may_carry_loans` is conservative for
    a struct whose declared fields have parameter types, so
    `List[DictEntry[…]]` refuses today.
13. **Reference calls.** A `closed_reference_contract` is a
    `closed_method_contract` but for the reference it returns, on a receiver
    that needs a place. The reference's origin is the callee's declared origin
    against the receiver's place, its mutability is the receiver binding's,
    and the interior generation's tag is chosen by the receiver's constructor:
    none reads a struct parameter, so an instance substitutes the referent and
    gets its own receiver back in the origin. The target is realized as in
    obligation 7. Inference marks a reference result a copyable read where its
    referent is implicitly copyable, whatever reads it, so the instance marks
    each again at its own referent: a template over a merely movable `T`
    marks nothing and its `Int` instance marks the read. A mark the template
    made must survive, since a by-value read rests on it; the struct's bound
    grants it, so that refusal is a guard and not a reachable verdict
    (`assets/type_error/template_method_reference_read_requires_copy.mojo` is
    the template's own rejection). `check_reference_result_reads` runs over
    every installed adjustment after the bodies, derived or not.
14. **Plain-data transfers.** A body that replayed a transfer summary
    (`vanishing_transfers`) derives only when every retained expression and
    binding type is closed and holds no loan, reference, or callable in its
    storage, its fields at their own arguments (`loan_free`; `plain_data`
    reads a struct's declared fields and would refuse every struct with a
    field of a parameter type). The template's own body, whose parameter is
    symbolic, never meets this and is inferred again. Obligation 9 then holds
    as before: the realized callee's summary must be empty.
15. **Comparisons.** Each admitted comparison is dispatched on the
    substituted operand type (`realize_comparison`): a closed scalar records
    nothing; a nominal struct records the dunder target
    `struct_infix_dispatch` selects, when the dunder is overloaded or a
    per-instantiation clone, and the operand's application as a receiver
    would. A dunder that converts or consumes its operand, `!=` served by
    `__eq__`, a built-in aggregate, and any other type refuse.
16. **Bound dispatches.** Each call the template dispatched through a bound
    is re-selected on the substituted receiver type (`realize_bound_dispatch`,
    through `bound_witness`): a built-in value's `copy`
    (`builtin_copy_is_value_read`) loses its contract, target, and parameters
    and marks the receiver a copied place; a built-in leaf's `__hash__`
    (`builtin_hashable_ty`) loses the same three and records the leaf; a
    nominal struct's witness is the lone declaration of the requirement's
    name whose shape is the requirement's — a read `self`, one parameter per
    argument with the recorded convention, no variadic, default, `raises`, or
    reference result — and the contract takes its target (the instance clone
    where one exists), its result and parameter types under `Self`, the
    struct's arguments, and the witness's own binders, and the call
    parameters the abstract call recorded empty. A binder of the witness
    (`String.__hash__[H]`) is admitted where exactly one parameter is that
    binder and the argument there is itself a bare parameter, so the binding
    stays symbolic and the per-call request it records (`MethodInstantiations`)
    is one discovery leaves alone. An inverted write whose receiver the
    instance makes a struct other than `String` becomes the struct's own
    `write_to` call the same way (`realize_inverted_writes`): the adjustment
    and the receiver's borrow go, and the writer becomes a kept place with
    its generation refresh. An overloaded requirement, a witness of another
    shape, a binder an instance would bake, and a type that is neither a
    built-in nor a declared struct refuse. Bound dispatches are realized
    before the closed calls, which then leave a nominal target as it stands.
17. **Bound builtins.** Each `hasher.update(x)` must find its argument's
    substituted type `Hashable` (`is_hashable`, which records a wide SIMD
    leaf), each `_update_with_simd(x)` a vector, and each `writer.write(x…)`
    every argument's type printable (`printable_argument`, the acceptance
    `infer_print` applies). Nothing is recorded; the demand only refuses.
18. **Constructions.** Each admitted construction is re-selected on the
    substituted constructed type (`realize_construction`). A `copy:`
    construction and a fieldwise one select nothing; the fieldwise one's
    arguments must still equal their fields under the struct's own
    substitution (a reference field's argument its referent). A hand-written
    family keeps the template's member — the one whose lowered name the
    template recorded, or the lone declaration — whose parameters every
    argument still binds exactly (`match_call_slots` on the recorded
    argument types; a literal materializes to a closed parameter; a
    defaulted parameter has a closed type), unless every member's parameters
    are closed, and takes the instance's clone of that member
    (`constructor_clone_target`) as its target. A member with binders of its
    own, a variadic binding, and a family the instance collapses refuse. A
    bundled struct mints no constructor clone, so the template's spelling
    stands and the instance owes the member's `where` clause
    (`method_constraints_apply`) instead.

The declaration's bounds and `where` clauses are not re-checked: the checker
discharges them at the requesting call, and the elaborator proves a fully
bound clone's `where` predicates before it generates the clone.

**A refusal is never a verdict.** A failed obligation refuses the derivation,
and the clone check then reports the failure in its own words. The fallback is
the strict check, so a false `rebind` still reads "rebind: the input type does
not match the result type" and a missing `len` witness still reads as it did.

## Overload selection is bound once

The pinned Mojo binds a call inside a generic body while it checks the body.
`pick(x)` with `x: T` can only select `pick[T]`, and `outer(3)` therefore
prints 2, not the 1 that re-ranking the `Int` clone selects. A derived instance
inherits the template's choice: a call through an overload set keeps the
member whose lowered symbol the template recorded, and is never ranked again.

An instance that still takes the clone check re-ranks. A `def` with a scalar
local now derives (`assets/ok/template_overload_binding_local.mojo`); one that
binds a local of a parameter type does not, and is the residue filed in
roadmap section 3
(`conformance/probes/template_overload_rebound_in_clone.mojo`).

## Reuse across passes

A certified surviving template's body is served from its own retained facts in
every later transfer round and discovery round, under the identity
substitution. The pre-rekey identities are stable across rounds because every
round re-elaborates the same prepared source. A template whose callee
summaries changed is inferred again and re-recorded.

A declaration source validation recorded is, in an elaborated program, that
template's trapping stub. It is neither recorded nor reused.

## Verification mode

`MOJITO_VERIFY_TEMPLATE_FACTS=1` infers every derivable body as well, captures
what that inference recorded with the same routine, and compares the two
bundles. Both sides are keyed by pre-rekey identity and template-local
bindings, which is the explicit bijection. Nothing else is normalized.

Two differences are expected and are the only ones accepted:

- A clone check never selects a `rebind`'s by-value overload, so that flag is
  cleared on both sides.
- A clone check re-ranks an overload set the template already selected from.
  `overload_rebinding_only` accepts a difference confined to the selection
  facts of such a call, and the derived facts then replace the inferred ones.

Any other mismatch is an `InvariantViolation`. The mode costs one extra
inference per derivable body and is excluded from measurements.

## Discovery without the arena

`check_program_for_discovery` runs every rejecting check and returns a
`DiscoveryResult`: `CheckedProgram::new`'s inputs, owned. The driver's six
request collectors read it directly. The two that walked the arena use
`DiscoveryResult::scan_expressions`, which is the arena builder's own
traversal with node construction switched off, so they see exactly the
expressions and types the arena would hold
(`discovery_scan_matches_the_checked_arena`). Only the round that converges is
finalized.

## Diagnostics

Nothing about an uncovered body's diagnostics changed. A covered ill-typed
body already failed at the template, in source validation or the abstract
check. A covered failed obligation refuses and falls to the clone check's
text. A violated `where` clause is reported by the elaborator or the requesting
call, as before.

`--timings` reports `body_inference.{plain,template,clone}`,
`templates.{surviving_trait_bound,validated_keyed,concrete_only,validation_aborted}`,
`template_facts_recorded`, `template_fact_entries`,
`template_capture_incomplete.*`, `template_derivations.{installed,ineligible,verified,overload_rebinding}`,
`template_bodies.reused`, `body_sites.instance_clone`, `template_census.*`, and
`arena_builds`. `MOJITO_TIMING_NOTES=1` adds `note` lines naming each
declaration, what a generic body recorded, and what keeps it from being
captured. `TemplateStats::refused` carries each refusal and its reason.
A verification mismatch names only the fields that differ
(`CheckedBodyFacts::difference`).

## Measured coverage, 2026-09-21

`docs/performance.md` (*Checked templates and the discovery result*) has the
tables. In short, for one debug-profile run each:

- Building the arena once saves about 6% (Hello World 10.58 s to 9.90 s).
- `stdlib_heavy.mojo` checks a per-instantiation method clone 2254 times per
  compilation and derives 802 of them (36%); `generic.mojo` derives 256 of
  474 (54%). With `MethodScalarBody` alone those were 242 and 70, and wall
  time did not move. With `MethodBody` it does: 17.9 s to 17.0 s and 11.4 s to
  10.6 s, three interleaved runs each. `ref self` accessors, reference
  results, and the `_mojito_abort` statement took the counts from 586 and 178.
  `ref` locals, receivers reached through a reference, and `mut`/`ref`
  parameters moved neither count: every bundled body that holds one also
  constructs a struct, passes a non-scalar, or dispatches through a bound.
  They derive in user structs today
  (`assets/ok/template_method_reference_{local,receiver}.mojo`,
  `template_method_borrowed_parameter.mojo`).
  Subscript stores, place arguments, and origin-bearing parameters took the
  counts from 698 and 220: the bundled gain is the `write_repr_to` forwarders
  that hand their `mut writer` on, and the rest is user structs
  (`template_method_{subscript_store,place_argument,origin_parameter}.mojo`).
  Whole-value arguments, vanishing transfers, comparisons, and bound copies
  took them from 710 and 226 to 802 and 256: `Dict.__contains__`,
  `List.__contains__`, `count`, `try_index`, `__eq__`, `__ne__`, `pop`,
  `__iadd__`, and `extend(Span)` derive
  (`template_method_{value_argument,vanishing_transfer,operator_dispatch,bound_copy}.mojo`).
  Bound dispatches with arguments, the two builtins, and the method's own
  hasher binder took `stdlib_heavy` from 802 to 860: `List.__hash__`,
  `List.write_to`, `Dict.write_to`, and `Optional.write_to` derive for every
  instance, and `Set.write_to` is certified (no instance reaches it there).
  A user struct that hashes or writes through its parameter derives for an
  `Int`, a `String`, and a user-struct instance alike
  (`template_method_bound_dispatch.mojo`).
  Constructions took `stdlib_heavy` from 860 to 1004 and `generic.mojo` from
  274 to 310: `List.copy`, `Optional.copy`, `Dict.copy`, `List.try_index`,
  `Dict._reset_index`, and the four iterator makers (`List.__iter__`,
  `List.__reversed__`, `Optional.__iter__`, `Dict.take_items`) derive. A user
  struct's `copy:`, fieldwise, collection, and hand-written constructions
  derive with the constructor clone retargeted
  (`template_method_construction.mojo`), and a view constructed over
  `ref source = self` with the receiver in its origin argument does too
  (`assets/extensions/ok/ref_field_template_method_construction.mojo`).
- Hello World mints no per-instantiation clones at all. Its generated bodies
  are members of structs specialized whole and per-call clones, from
  concrete-only templates.
- The census (`template_census.*`) says which table recipes come next: of
  the 260 clone bodies still inferred, 173 are capturable already, and the
  largest blocker left is a call-through residue (39 bodies, sole blocker of
  27), then `ExplicitDestroyCalls` (13), the `TypeName` adjustment (11), and
  `BorrowViewResult` (6, the `Dict.keys`/`values`/`__iter__` family). Its
  `grammar.*` counters say which constructs keep a body outside every class
  whatever its tables: a method call with arguments (150 bodies), a direct
  call with arguments (145), a subscript that is neither a pointer slot nor
  a reference call on a field (107), and a non-scalar closed result (90).

An earlier version of the `body_inference.clone` counter tested for a `$` in a
name and so counted ordinary bundled structs' methods as clones. The figures
it produced (3828 for Hello World) were wrong and are withdrawn.

## What is not covered

Each of these keeps the clone check. The roadmap carries one entry per item.

- A method body beyond `MethodBody`: a receiver origin, a copy or move
  initializer, `raises`, the method's own binders, a `for` loop, a string
  other than the `_mojito_abort` message, a call passing a `ref` local or a
  reference call's result, an argument that converts, and a local whose type
  is built over a parameter (`var result = List[Self.T]()` in
  `List.__mul__`, `List.__getitem__(slice)`, and the owned `__iter__`s).
- A construction with an `ImmOrigin` or a value compile-time argument, one
  whose constructor has binders of its own or binds a variadic parameter,
  one whose argument is a `ref` local bound to anything but a fieldwise
  struct's reference field or is a reference call's result, and a sibling
  call returning a view (`_DictKeyIter(self.items())` records
  `BorrowViewResult`). A pointer's own place provenance inside a retained
  type is still refused; only a struct's origin arguments are kept by
  template owner.
- A reference used as an argument, as the receiver of a method its bare
  parameter type promises through a bound, or as the base of a whole store
  (`self.entries[i].hits = 0` records a subscript descriptor), and one yielded
  by a callee on anything but a field of `self`. A struct with an origin or a
  value parameter (`Span`, `Array`) derives no method at all.
- An instance whose argument may carry a loan, which includes every struct
  with a field of a parameter type (`DictEntry[K, V, H]`).
- Per-call method clones and members of a struct specialized whole, which
  leave no trace.
- A local of a parameter type, a `for` loop, or a non-scalar result in a
  surviving trait-bound `def` template. Scalar locals and runtime `if` and
  `while` are covered.
- Any call that records a conversion, an adjustment, or an origin, and a
  transfer that does not vanish: a call-through residue (a body that calls
  its own callable parameter, `List.deinit_with`, `Optional.map`, and their
  callers), a function value's baked effects, a destination a captured
  binding names, and an instance whose values may carry a loan. An
  arithmetic operator through a bound, a reflected or consuming operator,
  and `!=` served by `__eq__` are re-selections in a clone with no recipe.
- A bound dispatch whose instance witness is overloaded, has a `mut` or
  consuming receiver, takes a binder the instance would bake (a closed
  hasher type), or is a requirement the type meets without declaring the
  method (a struct's reflective `__hash__` default); and a
  `hasher.update(x)` whose receiver is a concrete hasher, which selects a
  callee. `Dict.__hash__` constructs its `H2()` (`ConstructTypeParam`) and
  `Optional.__hash__` a `UInt8` tag (`SimdConstructions`), so both keep the
  clone check for those reasons.
- A folded value parameter or loop variable that survives into an instance.
- A local declared inside a `comptime for`.
- Packs, `DType` and vector parameters, reflection, struct-valued parameters,
  and anything a validation run that ended without a verdict reached.

The persistent elaboration session and the expansion worklist (plan slices 8a
and 8b) did not land. Measurement says why: the elaborator's per-round
invariant state costs about 5 ms, and elaboration is about 2.5% of a
compilation. The cost is body inference of uncovered bodies in every transfer
round, which a scheduler does not reduce.

## Re-homing

A `CheckedTemplate`'s binders, body, obligations, and selected call recipes
are the data a function generator would hold, and an `InstanceTrace` plus its
substitution is a generator-plus-parameters expansion node. Scoped
substitution would move to the parameter-expression layer's context. Source
provenance, conformance policy, call binding, diagnostics, CTFE fuel, and the
backend contract stay. This note defines no dialect spelling.
