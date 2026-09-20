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
(`MaterializeLiteral` only), generic instantiations, overload targets, call
parameters, selected calls (a `trivial_method_contract` only), borrowed read
call places, read temporary arguments, unconsumed temporaries, deletable
bindings, copied places, interior invalidations, and rebind assertions.

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

Locals are admitted only in a keyed body and never inside a `comptime for`: a
local declared in an unrolled body would need one binding per copy, and no
recipe mints those yet. The loop variable may only key a condition.

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
7. **Trivial method calls.** `trivial_method_contract` names every field of
   `CheckedCallContract`: no arguments, a plain read receiver, no raise, no
   result adapter, no reference result, no captures, no compile-time
   parameters, a closed result. `realize_method_call` then repeats the clone
   check's retarget, `instance_method_clone`, and writes the clone as the
   call's target, its overload target, and its effect-summary key. The callee
   must be the one method of its name, with no binders and no availability
   condition. An instance that has clones but not this one (withheld, or a
   collapsed overload family) refuses.
8. **Struct applications.** Substituted, then recorded by installation under
   the body's own source.
9. **Effect summaries.** Every callee's transfer and call-through summaries
   must still be empty. Installation records the same empty observation a
   clone check would, so the transfer fixpoint re-runs if one grows.

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

An instance that still takes the clone check re-ranks. That is the residue
filed in roadmap section 3
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
  compilation and derives 242 of them (10.7%); `generic.mojo` derives 70 of
  474. Wall time is unchanged within noise, because the bodies that derive
  today are the smallest.
- Hello World mints no per-instantiation clones at all. Its generated bodies
  are members of structs specialized whole and per-call clones, from
  concrete-only templates.
- The census (`template_census.*`) says three recipes —
  `DiscardedReferenceResults`, non-trivial `SelectedCalls`, and
  `ConstructionImmutableBinders` — would make 137 of 392 clone bodies
  capturable, and adding `PointerOffset` and `PointerStorageTake` 190.

An earlier version of the `body_inference.clone` counter tested for a `$` in a
name and so counted ordinary bundled structs' methods as clones. The figures
it produced (3828 for Hello World) were wrong and are withdrawn.

## What is not covered

Each of these keeps the clone check. The roadmap carries one entry per item.

- A method body beyond `MethodScalarBody`: a non-scalar result, a `mut` or
  consuming receiver, a lifecycle method, a call with arguments, a local, a
  branch, or a loop.
- Per-call method clones and members of a struct specialized whole, which
  leave no trace.
- Bodies with locals, control flow, or non-scalar results in a surviving
  trait-bound `def` template.
- Any call that records a conversion, an adjustment, an origin, a transfer, or
  a contract that is not trivial. A `T: Bound` receiver is a re-selection in a
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
