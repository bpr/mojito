# Deriving instantiations from checked templates

Mojo checks a parametric body once, with its parameters symbolic, and
instantiation substitutes into the checked result. Mojito used to do the
reverse: the elaborator cloned a body per instantiation and the checker
inferred every clone from scratch, once per discovery round and once per
transfer-effect round. This note records the mechanism that makes a checked
template the authority for the instantiations it covers, what it covers
today, and what it does not.

The plan this implements is `instantiation-from-template-plan.md` (untracked,
repository root). The roadmap entry is the first checkbox of
`docs/roadmap.md` section 1.

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
`PointerStorageDestroy`, and a moving `PointerWrite`), generic instantiations,
overload targets, call parameters, selected calls (a `closed_method_contract`
or a `closed_reference_contract` only), borrowed read call places, read
temporary arguments, unconsumed temporaries, discarded results, deletable and
linear bindings, linear temporaries, copied places, interior invalidations,
rebind assertions, reference handles (`ReferenceValueUses`, at the value of a
`return` in a method that returns a reference, and nowhere else), reference
results (the `ReferenceResult` adjustment, kept apart from the other
adjustments), interior references, and copyable reference-result reads.

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
`mut`, and bare `ref` parameters, and any result, a reference included. A
`mut` or `ref` parameter is bound from its declared convention and rooted at
its own binding under every instance, so the body's facts name it by template
owner; what its caller owes lives in the signature, which is checked per
clone. The body may store to a `mut` parameter, a scalar or a whole value of
the parameter's own type, and may only read a bare `ref` one, which it never
moves out of. Which fields an `__init__` initializes is
its syntax, and definite initialization is judged outside the body check. A
bare `ref self` is a receiver like the others: it has parametric mutability, so
the body cannot write through it, and one binding identity names it under
every instance. A receiver or parameter origin, the copy and move
initializers, the method's own binders, and `raises` stay outside. A `where`
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
   supplied and bound by value to a closed scalar parameter, adapted at most by
   materializing a literal to a closed type, and no raise, result adapter,
   reference result, captures, or compile-time parameters. A
   `trivial_method_contract` is the case with no arguments, a plain read
   receiver, and a closed result, which is all `MethodScalarBody` admits.
   `realize_method_call` repeats the clone check's retarget: the declared
   member is the one whose lowered name the template recorded, and the clone
   member is the one with that signature (`method_clone_target`, shared with
   `constructor_clone_target`), giving `List.pop$y3:Int$ov$Int`. A clone check
   ranks the clone family again on its arguments, so every member of an
   overloaded family must declare closed parameter types for the two rankings
   to agree. The result type substitutes. A method call's parameter types are
   already in the receiver's binder scope, unlike a direct call's
   `CallParameterFact`, so they would substitute too if they were ever
   symbolic. A clone that exists has met its `where` clauses; a callee with
   an availability condition and no clone refuses, as does an instance that
   has clones but not this one (withheld, or a collapsed overload family).
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

## Measured coverage, 2026-09-20

`docs/performance.md` (*Checked templates and the discovery result*) has the
tables. In short, for one debug-profile run each:

- Building the arena once saves about 6% (Hello World 10.58 s to 9.90 s).
- `stdlib_heavy.mojo` checks a per-instantiation method clone 2254 times per
  compilation and derives 698 of them (31%); `generic.mojo` derives 220 of
  474 (46%). With `MethodScalarBody` alone those were 242 and 70, and wall
  time did not move. With `MethodBody` it does: 17.9 s to 17.0 s and 11.4 s to
  10.6 s, three interleaved runs each. `ref self` accessors, reference
  results, and the `_mojito_abort` statement took the counts from 586 and 178.
  `ref` locals, receivers reached through a reference, and `mut`/`ref`
  parameters moved neither count: every bundled body that holds one also
  constructs a struct, passes a non-scalar, or dispatches through a bound.
  They derive in user structs today
  (`assets/ok/template_method_reference_{local,receiver}.mojo`,
  `template_method_borrowed_parameter.mojo`).
- Hello World mints no per-instantiation clones at all. Its generated bodies
  are members of structs specialized whole and per-call clones, from
  concrete-only templates.
- The census (`template_census.*`) says which table recipes come next:
  `ConstructionImmutableBinders` alone would make 56 more of the 314 clone
  bodies still inferred capturable, and a callee effect summary that is not
  empty 27 more. Its `grammar.*` counters say which constructs keep a body
  outside every class whatever its tables: a method call passing something
  other than a scalar (175 bodies), a direct call with arguments (174), a
  subscript that is neither a pointer slot nor a reference call on a field
  (129), and a non-scalar closed result (117).

An earlier version of the `body_inference.clone` counter tested for a `$` in a
name and so counted ordinary bundled structs' methods as clones. The figures
it produced (3828 for Hello World) were wrong and are withdrawn.

## What is not covered

Each of these keeps the clone check. The roadmap carries one entry per item.

- A method body beyond `MethodBody`: a receiver origin, a copy or move
  initializer, `raises`, the method's own binders, a parameter with an origin
  clause, a `for` loop, a construction, a string other than the `_mojito_abort`
  message, a call passing a value that is not a closed scalar, and a local
  whose type is built over a parameter.
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
- Any call that records a conversion, an adjustment, an origin, a transfer, or
  a contract that is not closed. A `T: Bound` receiver is a re-selection in a
  clone (conformer union in the template, concrete dispatch in the clone), not
  a substitution.
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
