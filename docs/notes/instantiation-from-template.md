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
- A callee effect summary that was read and was not empty refuses it, unless
  what was read is a call-through residue, which the bundle keeps
  (`call_through_reads`).

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
owes that none of its values can. An operator over two parameter-typed
places records nothing in the template, so the grammar names it (`operators`)
and an instance dispatches it itself. A checker builtin on a bounded parameter
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

A view a call returns (`self.items()` over `-> View[origin_of(self)]`) names
the receiver in a struct origin argument instead. The contract's result type
keeps those slots unbound, their origins kept beside it by template owner
(`TemplateCallContract::result_origins`), as a retained type's are
(`typed_origins`); the slots the call's contract resolved
(`CallResultOrigins`) are kept the same way (`call_result_origins`); and the
call's `BorrowViewResult` loan names nothing an instance changes. A struct
application's origin arguments are kept unbound too, since recording one
erases them.

## The expansion trace

Two halves identify what a clone's occurrence came from. Neither demangles a
symbol.

- **Declaration level.** `generate_def_spec` records a `DefInstanceTrace` per
  clone: the prepared declaration it instantiates, the source type written for
  each baked type parameter, each folded value parameter, the source element
  types written for each baked type pack, and the parameters the clone still
  declares. The driver converts these to `InstanceTrace` and hands them to
  the catalog before each check.
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
writes a fresh literal where the identifier stood. A folded `comptime for`
variable is not: the literal keeps the identifier's identity, takes a
literal's facts (its own type, nothing else) rather than the variable's, and
where it indexes a pack it says which element the copy is — the template's
`Ts[i]` at that occurrence is replaced under `i` bound to the literal and the
pack bound to its element list (`substitute_packs`), and folds to the
element.

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
| `FixedCalls` | plus direct calls of module-scope functions, passing literals, parameters, and further such calls | Selection is retained. No copy or adjustment was recorded, so every argument either matched its parameter exactly and still does after substitution, or converts through an `@implicit` constructor the instance selects again (obligation 20), which never re-ranks the callee. Borrows depend on slots and conventions, not types. The callee's effect summaries were empty and are re-read. |
| `BoundedOperations` | plus the built-in `len` over a parameter whose bound promises a length | The bound proved the call. The instance owes the witness, which `len_result_for_type` finds, and takes the read-in-place fact `infer_len` adds for a nominal struct. |
| `MethodScalarBody` | a method with a plain read `self` and no binders of its own, on a struct of plain type parameters: `return`s over closed scalars, parameters, reads of `self`'s scalar fields, the built-in `len` over a field, and argument-free method calls on `self` or a field with a trivial contract | A field read has the field's declared type under the struct's arguments in a template and a clone alike. The generated-declaration leniency a clone's name switches on bears on origin-bearing return annotations only, and the result is a scalar. A trivial call can change per instance only in its target. |
| `ScalarBranches` | source-validated bodies: `comptime if` arms, scalar `comptime for` loops, scalar locals and assignments, erased `rebind`s | Every arm was checked once. The instance keeps the occurrences the elaborator selected. |
| `PackElements` | a source-validated body keyed on a type pack (`*Ts`), collected by one variadic parameter: `comptime for` over the pack's indices, each element read as `pack[i]` into a `print` statement, beside what `ScalarBranches` admits; the result may be `None` | The element was checked once at the dependent `Ts[i]` through the pack's bound, and recorded as a place read where it lies. Each unrolled copy carries the folded index, so the instance fixes the element per copy; `print` selects no callee and records at an argument only what its syntax decides, and the instance proves the fixed element `Writable` again (obligation 21). A pack forwarded whole, a loop variable used anywhere but as the index, and a local inside the loop stay out. |
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
`mut`, bare `ref`, and `def(...)`-typed parameters, trait-bounded type
binders of its own, and any result, a reference included. A `deinit`
receiver is writable as a `mut` one is. A
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
copy and move initializers, and any other binder stay outside. A `where`
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
| `SIBLING_CALLS` | a method call on `self`, a field of it, or a `var` local, passing closed scalars, whose contract is a `closed_method_contract`; its result may be a view over the receiver (`_DictKeyIter(self.items())`) | Obligation 7 below. Arguments are closed scalars in both checks, so nothing about them depends on the instance. A view result's `BorrowViewResult` loan and the origins its contract resolves are the callee's declared `origin_of(self)` return over the call's own receiver, which no instance changes; a constructor the view is handed to matches its field or parameter but for those origin arguments. |
| `REFERENCE_RESULT` | a method that returns a reference: every `return` hands out a field of `self`, a pointer slot, or a reference call's result, of exactly the declared referent type | The `return` keeps its value as a handle because the declaration returns a reference (`ReferenceValueUses`, written from the declaration and the statement's syntax alone) and demands neither a copy nor a move. Whether the place lies within the declared origin is judged on its path, the signature, and the receiver's constructor, none of which an instance changes; the loan-escape check reads what obligation 12 already rules out. The signature is checked per clone before the body, derived or not. Any other handle in the body refuses it. |
| `REFERENCE_CALLS` | a subscript or a named accessor on a field of `self`, passing closed scalars, whose contract is a `closed_reference_contract`, either returned as above or read by value into the result or a `var` | Obligation 13 below. The call is never a receiver, an operand, a condition, or an argument, each of which records a borrow of its own. A by-value read is admitted only where the template marked the read copyable. An iterator's `__next__` marks its read by another rule and stays out. |
| `REFERENCE_LOCALS` | `ref name = place`, where the place is `self`, a field of it, a parameter, a `var` local, or a reference call; then a field read through the binding, the binding copied out whole, a scalar operand, `len` over it, a scalar store through it, or the binding forwarded as the method's own reference result | The declaration decides mutability from the value's reference and the binding it names, re-stamps an origin computed upstream, and runs no check of its own. Its type is a reference whose origin names a binding, so a bundle keeps it by template owner, as it keeps a call's, and an instance substitutes the referent. Every use records the referent. A copy out owes obligation 3, and a copyable read obligation 13. |
| `REFERENCE_RECEIVERS` | a field read or a closed method call through a reference call's result or a `ref` local whose referent is a struct | A call borrows such a receiver (`BorrowedReferenceReceivers`) because of what the receiver is: a reference result, a `ref` binding, a `ref` field. None of those tests reads the referent. The callee is realized as in obligation 7, from the receiver's own substituted type; a receiver whose type was already closed in the template (`List[Pair]`) selected its clone there. A method of a bare parameter dispatches through the bound and stays out. |
| `SUBSCRIPT_STORES` | a store through a subscript of a writable `self` or of one of its fields: a closed scalar field of the element a reference getter yields (`self.entries[i].hits = 0`, `+= 1`); an element a declared setter takes, a closed scalar or a whole value of the setter's own parameter type (`self.counts[i] = n`, `self.items[i] = value^`, `self.entries[i] = Entry[Self.T](v^)`, `self[k] = v^`); a closed scalar element stored whole or augmented through the mutable reference its getter yields (`self.counts[i] += 1`, `self.grid[i] = n` on a struct with no setter) or augmented through a closed value getter and a setter (`self.table[i] += 1`); or an element of a closed struct type stored augmented through its in-place dunder (`self.counters[i] += 3`) | A subscript that is a place records its index shape and whether the setter takes the value by keyword (`SubscriptDescriptors`). The first is the syntax, one plain index; the second is the setter's declaration. The getter is a `REFERENCE_CALLS` call and the setter a `SIBLING_CALLS` one, recorded at the subscript and realized by the setter's name; its index and value are the call's arguments (`VALUE_ARGUMENTS`), so a whole value binds a parameter of exactly its own type and owes what it owes at any call (obligations 3, 10, and 14). A store through a mutable reference records no second contract: the checker writes the computed value back through the getter's reference, and its `AugmentedSubscript` record is that getter beside the element's type, kept apart from the adjustment table (`augmented_subscripts`, as `reference_results` are) and rebuilt for an instance from the getter it realized at the site. A store through a value getter and a setter records the setter at the subscript, and the checker keys the computed value it binds there too, since that value is no expression; its `AugmentedSubscript` record keeps the value getter beside the setter in `augmented_subscripts`, as it stands, so the subscripted value's type is closed. A struct element's `+=` selects its `__iadd__` on a synthesized receiver whose facts and identity the checker drops with its scope; the dunder is kept beside the store likewise, over a closed element type. A getter an instance would retarget to its own clone, or a dunder selected through a bound, stays out. A whole element read from the same list (`self.items[i] = self.items[j]`) is accepted for `Int` and refused for `String`, so it stays out until that divergence closes. |
| `PLACE_ARGUMENTS` | a local, a parameter, or a field of `self` handed to a `mut` or bare `ref` parameter of a method call | The callee's declared convention decides that the call keeps the caller's place (`CallPlaceUses`), and the generations a `mut` argument invalidates lie below the argument's own binding, kept by template owner. A kept place is neither copied, moved, nor converted, and its recorded type equals the parameter's. Whether two arguments conflict is judged on their places and conventions. A field of `self` is kept only beside a receiver the call reads. A parameter of a struct parameter's type is admitted only on a call of `self`'s own method, where callee and caller share one binder scope, so the instance substitutes it in the contract and in `CallParameters`. |
| `ORIGIN_PARAMETERS` | a `ref` parameter with an origin clause, the method's origin binders, and the parameter forwarded as the method's own reference result | The clause lives in the signature, which is checked per clone. The body's facts for such a parameter are a bare `ref` one's: its binding, its type, and a copy where it is read by value. |
| `VALUE_ARGUMENTS` | a whole value of any type handed to a by-value parameter of a method call: a `^` transfer, a sibling call's result, a place the template copied, or a named place a read parameter takes where it lies | The argument's recorded type equals the parameter's, before and after substitution, or an `@implicit` constructor converts it to the parameter's, and the instance selects that constructor again (obligation 20). What the call records for it is decided without its type: a read parameter borrows a named place and reads a temporary by the argument's syntax and the callee's conventions, and a `var` parameter takes a transfer or a temporary as it stands. A copied place owes obligation 3 and a transfer obligation 10, as elsewhere. A `ref` local and a reference call's result stay out, since each records a borrow of its own. A callee with a parameter of a struct parameter's type may belong to a field of another struct: it has no binders of its own, so its parameter types were recorded at the receiver's arguments, in the caller's binder scope, and the instance substitutes them in the contract and in `CallParameters` alike. An overloaded family whose members declare parameters of parameter types is admitted when every argument's type is exactly its parameter's: no member outranks an exact match, and a family an instance collapses finds no single clone (`method_clone_target`). |
| `VANISHING_TRANSFERS` | a call whose callee stores an argument outward (`self.items.append(value^)`), so the template replays a transfer summary at it | A transfer moves the loans its source carries. The template's parameter may carry one (`type_may_carry_loans` is true for a symbolic parameter), so its check records a call transfer, merges the origin, and publishes an effect of its own; every value of a plain-data instance carries none, so `replay_transfer_effects` records nothing and the instance's frame publishes nothing, which is what the clone check records. Obligation 14 below. A call-through residue, a function value's baked effects, and a destination a captured binding names are not transfers and still refuse. |
| `OPERATOR_DISPATCH` | Every operator a trait names — `==`, `!=`, `<`, `<=`, `>`, `>=`, and the arithmetic, bitwise, and shift ones — over two places of one type that mentions a struct parameter | The template proves the operator through the bound and records nothing at it; `infer_infix` decides a struct operand's dunder from the operand types alone (`struct_infix_dispatch`). Obligation 15 below. Each operand is a place, so both checks read it where it lies; what the instance's dispatch adds beneath the operand is the copy of a consumed one, the conversion of an adapted one, and `NegatedEquality`. An arithmetic operator's result is the operand's own type rather than `Bool`, so it is a temporary of that type wherever the body puts it (`BodyShape::operator_value`, beside a call result and a construction). |
| `BOUND_DISPATCH` | a method call on a place of a bare parameter type, or on a named place's `^` transfer for a `var self` requirement, proved through a bound: `place.copy()`, `item.__hash__(hasher)`, `item.write_to(writer)`, `item.bump(2)` on a `mut self` requirement, or any requirement whose arguments are closed scalars or named places handed to a bounded `mut`/`ref` parameter, each of a bare parameter type or of a closed type the template proved against the bound | The template records the abstract dispatch (`__trait_dispatch.__hash__$ov$…`), or for `write_to` the inverted write, and nothing the receiver's type decides: a kept argument's place use and generation refresh follow from the requirement's convention, which the witness shares. `infer_method_call` decides a concrete receiver's witness from its type alone, and `bound_witness` repeats that decision from the types. Obligation 16 below. The call through the bound read the summaries of every conformer's method of that name, one key per conformer; each was empty, and the instance re-reads only its own target's. |
| `BOUND_BUILTINS` | `hasher.update(x)`, `hasher._update_with_simd(x)`, or `writer.write(x…)` on a parameter bounded by `Hasher` or `Writer`, whose arguments are closed scalars, string literals, named whole values, `ref` locals, fields read through a reference, pointer slots, or reference calls | The builtin selects no callee and records at an argument only what its syntax decides: a borrow of a named place or a reference result, an unconsumed temporary, a closed literal's materialization. The argument's type it proved through the bound, and the instance proves it again at its own type (obligation 17), which for a hashed value is where the hash leaf is recorded. |
| `BOUND_BINDERS` | the method's own type binders, each a plain trait-bounded type (`[H: Hasher]`) | A clone keeps such a binder, bound symbolically as the template binds it: no clone is minted per hasher, the VM reifies the binding at run time, and no retained fact substitutes it. The instance's substitution binds the struct's parameters alone (`instance_substitution`), as it does beside an origin binder. |
| `CONSTRUCTIONS` | a construction of a declared struct whose compile-time arguments are types, as a whole value: `copy:` of a named place, or arguments that are closed scalars, whole values, a place lent to a hand-written constructor's `ref` parameter (`REFERENCE_ARGUMENTS`), or — for a fieldwise struct's reference field — a `ref` local | A construction records no contract: `infer_construction` selects an `__init__` from the argument types, retargets it to the instance's constructor clone, and reaches the struct application. Its one unconditional record, `ConstructionImmutableBinders`, is empty when no compile-time argument is an `ImmOrigin` cast, which the type-only arguments guarantee, and installation writes the empty entry again. What a constructor records at an argument is decided by the argument's syntax and the constructor's conventions (a copy owes obligation 3, a transfer obligation 10, a literal materializes to a closed type); the selected member binds each argument exactly, so no member can outrank it under any instance (obligation 18). A constructed type that names the receiver in an origin argument (`_ListIter[T, origin_of(self)]`) is kept with the slot unbound and the origin by template owner (`typed_origins`), and the `return`'s re-resolution of the annotation's `origin_of(self)` is repeated once by the instance rather than recorded. |
| `CALLABLE_PARAMETERS` | a call through a parameter declared with a `def(...)` type, passing closed scalars or whole values, as a statement; and such a parameter forwarded by value to a sibling call | The call records the parameter's own contract symbol and parameters, which the instance takes from its own binding of the parameter, and puts a call-through residue on the body's frame that names the parameter's slot and each argument's signature place. A forwarded parameter reads the callee's residue and composes one. Neither names a type: the instance republishes the template's residue and owes that its realized callee publishes the one the template read. Obligation 19 below. |
| `STRING_BUILTINS` | `_unqualified_type_name[T]()`, and `repr(value)` over an argument a checker builtin reads where it lies | Neither selects a callee. The reflection call records one type's spelling, which `derive_adjustment` re-renders from the substituted type (`TypeName` carries the type beside the text) and refuses while that type is still symbolic. `repr` proves its argument `Writable` — which the instance proves again at its own type — reads it where it lies as a bounded sink's argument is read, and wraps its compile-time string result as the nominal `String`: one conversion, whose literal constructor is the same under every instance (obligation 20).
| `REFERENCE_ARGUMENTS` | a `ref` local, a field reached through a reference, or a reference call's result handed to a method call's read, `var`, or `mut` parameter, or lent to a hand-written constructor's `ref` parameter | What the call records there is decided by what the argument is, as for a receiver reached through a reference: a read parameter borrows the place the argument names (`BorrowedReadCallPlaces`, by syntax), a `mut` parameter keeps it (`CallPlaceUses`), a `var` parameter copies it where the template recorded the copy (obligation 3), and a field read keeps its base as a handle. A reference call records its own result and interior, and its copyable-read mark is taken again at the instance's referent (obligation 13). A constructor's `BorrowRefArguments` names the lending positions, which are the selected constructor's declaration, and each loan's mutability, which is the place's; `derive_adjustment` carries it verbatim. One whose loan materializes a temporary names a binding of one run and refuses. |
| `RAISES` | a `raises` declaration, bare or typed, and a `raise` whose operand is a `CONSTRUCTIONS` construction or `Error("…")` of a string literal | A `raise` records nothing of its own. `require_error` asks two things of the operand's type: whether it is a string, which a constructed struct's name and the builtin `Error` settle whatever the instance, and whether it equals the declared error type, which it does under every substitution if it does symbolically, both types being written over the same parameters. The declared error type lives in the signature, checked per clone. A raised string literal, a raising call in the body (`effects_closed`), and a module-level `def` that raises stay outside. |

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
   must still be empty, except a callee the template read a call-through
   residue from, which obligation 19 covers. Installation records the same
   observation a clone check would — empty, or the residue read — so the
   transfer fixpoint re-runs if one grows. A name read where a callable
   stands as a value (a parameter forwarded, a declaration handed on) is read
   under the same name by the instance (`value_callees`).
10. **`Movable`.** Each `^` transfer of a value whose type mentioned a
    parameter must be of a `Movable` type for the instance. A parameter is
    always movable while it is symbolic and the demand only ever produces an
    error, so no retained fact carries it and verification cannot see it
    (`assets/type_error/template_method_transfer_requires_movable.mojo`).
11. **Deletability.** A binding whose type mentions a parameter is
    deletable, linear, or neither at the instance's own type, as the
    declaration's check decides it: deletable where the type is `Deinitable`,
    linear where it is still a bare parameter. A type built over a parameter
    (`List[T]`, `View[T, o]`) answers from its own conformance under the
    instance's arguments (`is_deinitable`). A linear temporary stays one only
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
15. **Operators.** Each admitted operator is dispatched on the substituted
    operand type (`realize_operator`): a closed scalar records nothing, owing
    only that the primitive path has the operation and gives the type the
    template kept (`scalar_operator_result`); a nominal struct records the
    dunder target `struct_infix_dispatch` selects, when the dunder is
    overloaded or a per-instantiation clone, and the operand's application as
    a receiver would, and its result must still be the type the template
    kept. A dunder that consumes its operand records the implicit copy of the
    place, and owes that the instance's type is implicitly copyable; one that
    converts it records the conversion, whose constructor obligation 20
    selects; `!=` served by `__eq__` records the `NegatedEquality`
    adjustment. A built-in aggregate and any other type refuse. The reflected
    dunder needs no arm: a comparison has no reflected form, and an
    arithmetic operator's left operand has the forward dunder its bound
    required.
16. **Bound dispatches.** Each call the template dispatched through a bound
    is re-selected on the substituted receiver type (`realize_bound_dispatch`,
    through `bound_witness`): a built-in value's `copy`
    (`builtin_copy_is_value_read`) loses its contract, target, and parameters
    and marks the receiver a copied place; a built-in leaf's `__hash__`
    (`builtin_hashable_ty`) loses the same three and records the leaf; a
    nominal struct's witness is the declaration of the requirement's name
    whose shape is the requirement's (`witness_binders`) — the receiver
    convention the abstract call recorded (read, `mut`, or `var`), one
    parameter per argument with the recorded convention, no variadic,
    default, `raises`, or reference result — and the contract takes its
    target (the instance clone where one exists), its result and parameter
    types under `Self`, the struct's arguments, and the witness's own
    binders, and the call parameters the abstract call recorded empty. A
    literal the requirement's closed parameter materialized keeps its
    materialization. Of an overload set, the one member that fits is taken,
    under its overload symbol (`method_lowered_name`), when no other member
    could take as many arguments (`takes_arity`); two members of one arity
    need a ranking on types and refuse. A binder of the witness
    (`String.__hash__[H]`) is admitted where exactly one parameter is that
    binder: a bare parameter argument keeps it symbolic, and the per-call
    request it records (`MethodInstantiations`) is one discovery leaves
    alone; a closed argument (a concrete hasher) bakes it, the request is one
    discovery serves, and once the elaborator has minted the per-call clone
    (`specialized_method_clone`) the call names that clone, as the clone
    check retargets to it. A `mut self` requirement keeps the receiver's
    place and generation refresh as the template recorded them, and a
    `var self` one is admitted only on a named place's `^` transfer, which
    records the move at the receiver itself. An inverted write whose receiver the
    instance makes a struct other than `String` becomes the struct's own
    `write_to` call the same way (`realize_inverted_writes`): the adjustment
    and the receiver's borrow go, and the writer becomes a kept place with
    its generation refresh. A witness of another shape, a binder baked into
    a generic struct's witness, and a type that is neither a built-in nor a
    declared struct refuse. Bound dispatches are realized
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
19. **Call-through residues.** A body that calls its own `def(...)`
    parameter records a `CallThroughEffect` on its frame, naming the
    parameter's slot and each argument's signature place; a body that
    forwards the parameter to a callee with such a residue reads it and
    composes one of its own. Neither names a type, so the instance
    republishes the template's residue verbatim (`call_throughs`, pushed on
    the instance's frame at installation) and rekeys each read
    (`call_through_reads`) to the callee obligation 7 or 5 realized
    (`note_realized_callee`), which must publish exactly the residue the
    template read. The ground is the same as obligation 14's: an argument
    carries an origin only while its binding's type carries a loan, so
    capture refuses a residue with a carried origin and realization refuses
    when a substituted retained type carries a loan or a reference
    (`residue_plain`; a callable's environment never substitutes, so a
    `thin` or open `capturing` parameter type is not refused as `loan_free`
    refuses it). The call through the parameter itself recorded the
    parameter's contract symbol (`__trait_dispatch.__call__$ov$…`) and
    parameters in the caller's binder scope, and the instance takes both from
    its own binding of the parameter (`realize_callable_call`). A residue
    naming a compile-time callable value refuses, and a read whose concrete
    callable is a named declaration with effects of its own keeps the clone
    check (`EffectRead::Residue`).
20. **Implicit conversions.** An `@implicit` constructor is selected from the
    source and the target type alone (`implicit_conversion_constructor`), so
    the instance repeats the selection at its own types rather than
    inheriting the constructor the template found, and records whichever
    member — or clone of a member — it names (`realize_conversion`). A
    substituted source that reaches the target without a conversion, that
    reaches it by none, or whose constructor consumes, raises, or borrows its
    source refuses: each of those makes the recorder do more than fill the
    four conversion tables. A conversion that kept no converted-to type is a
    nominal-string wrap, whose literal constructor no instance changes, and
    capture refuses every other target-only record. A conversion at a
    selected call's argument is also carried in the contract's own boundary,
    which capture and installation copy verbatim, so the instance writes the
    constructor it selected back into that second copy
    (`realize_boundary_conversions`); a boundary conversion whose own
    occurrence kept none refuses. Such an argument stands for its own type
    where the grammar demands the parameter's, and a module-level `def`
    template admits one too: its call's selection stands, as every call
    inside a generic body is bound once, and only the conversion beneath it
    is chosen again. A keyed body converts in a `comptime if` arm the same
    way: the instance re-selects only in the arms the elaborator kept. An
    annotated `var` converts its value the same way
    (`var label: Label[Self.T] = 4`, `var box: Wrapper[Self.T] = self.item`):
    the local holds the declared type, which the binding's recorded type
    substitutes, and a named place the constructor reads is neither copied
    nor moved, so it stands at the binding without a copy. The grammar
    admits an annotation only where the value's recorded type is the
    declared one or a conversion is kept at the value
    (`BodyShape::annotated_binding`): an annotation left to inference and a
    view that borrows its source (`var span: Span[Self.T, _] = self.items`)
    record neither, and stay out.

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
  (`template_method_bound_dispatch.mojo`), and so does one whose witness is
  a member of an overload set, takes a concrete hasher into a `[H: Hasher]`
  binder, or satisfies a `mut self` or `var self` requirement
  (`template_method_bound_witness_shapes.mojo`; `stdlib_heavy` is unmoved,
  since no bundled body has those shapes).
  Constructions took `stdlib_heavy` from 860 to 1004 and `generic.mojo` from
  274 to 310: `List.copy`, `Optional.copy`, `Dict.copy`, `List.try_index`,
  `Dict._reset_index`, and the four iterator makers (`List.__iter__`,
  `List.__reversed__`, `Optional.__iter__`, `Dict.take_items`) derive. A user
  struct's `copy:`, fieldwise, collection, and hand-written constructions
  derive with the constructor clone retargeted
  (`template_method_construction.mojo`), and a view constructed over
  `ref source = self` with the receiver in its origin argument does too
  (`assets/extensions/ok/ref_field_template_method_construction.mojo`).
  Call-through residues and a writable `deinit` receiver took `stdlib_heavy`
  from 1004 to 1044 (2026-09-22): `List.deinit_with`, `Optional.deinit_with`,
  and `DictEntry.reap_with` derive for every plain-data instance, and a user
  struct that calls or forwards its `def(...)` parameter derives for an `Int`
  and a `String` instance (`template_method_callable_parameter.mojo`). The two
  `List[DictEntry[…]]` instances still refuse on the plain-data obligation,
  `Dict.clear_with`/`deinit_with` on `ExplicitDestroyCalls` and a consumed
  local receiver, `Optional.map`/`and_then` on `EraseCompileTimeArgument`,
  and the `Tuple` teardown members are whole-struct specializations with a
  compile-time callable.
  The string builtins and the conversion recipe took `stdlib_heavy` from 1050
  to 1102 (2026-09-22): `List`, `Dict`, `Optional`, and `Set.write_repr_to`
  and `EmptyOptionalError.write_to`/`write_repr_to` certify and derive, and
  with them the `write_to` family beside them. A user struct that writes its
  own type name and a value's `repr` derives for an `Int` and a `String`
  instance (`template_method_string_builtins.mojo`), and one that binds a
  converting value re-selects the constructor at each instance
  (`template_method_converting_argument.mojo`). `Array.write_repr_to` reads a
  value parameter and `Tuple.write_repr_to` is compile-time keyed, so both
  stay out.
  Whole-value element stores and stores through a mutable-reference getter
  (2026-09-23) move neither count (1096 and 340 before and after): no
  bundled body holds `x[i] += …`, `Dict._append_new` is also kept out by a
  field read through a `var` parameter, an `Int(…)` conversion, and a
  method call on a local, and `Set.insert` is not instantiated by either
  benchmark. A user struct that stores a moved parameter, a moved closed
  value, or a construction through `List.__setitem__`, and one that stores
  or bumps an element through a mutable-reference getter, derive for an
  `Int` and a `String` instance (`template_method_subscript_store.mojo`).
  A converting argument at a call (2026-09-24) moves neither count: no
  bundled body converts an argument at a site the grammar otherwise admits.
  A user struct whose method hands a literal, and a value of the struct's
  parameter type, to a converting parameter derives for an `Int` and a
  `String` instance (`template_method_converting_call_argument.mojo`), and
  so does a module-level `def` template that converts a literal at a direct
  call (`template_def_converting_argument.mojo`).
  A conversion at an annotated binding (2026-09-24) moves neither count: no
  bundled body annotates a converting local. A user struct whose method
  converts a literal or a field of `self` into a local of a type built over
  its parameter derives for an `Int` and a `String` instance
  (`template_method_converting_binding.mojo`).
  A sibling call returning a view (2026-09-24) took `stdlib_heavy` from 1142
  to 1172 (`generic.mojo` stays at 352): `Dict.keys`, `Dict.values`, and
  `Dict.__iter__` derive for both `Dict` instances. A user struct whose
  methods return such a view, wrap it by a fieldwise or a hand-written
  constructor, spell it through a `comptime` alias, or bind it to a local
  derives for an `Int` and a `String` instance
  (`template_method_sibling_view.mojo`).
  A method that raises (2026-09-24) took `stdlib_heavy` from 1172 to 1224
  (200 to 209 distinct derived clones): `Optional.__getitem__` derives for
  the `Int` and `String` instances and `Dict.popitem` for both `Dict`
  instances. `Optional.__getitem__` over a `DictEntry` still refuses on the
  plain-data obligation, `Dict.pop` on `ExplicitDestroyCalls`,
  `Dict.__getitem__` on a reference result reached through an element's
  field, `List.index` on `Bool(result)`, and the iterators' `__next__` on
  their struct's origin parameter. A user struct whose accessors raise
  `Error("…")` or a construction of the declared error type derives for an
  `Int` and a `String` instance (`template_method_raises.mojo`).
  Widening `OPERATOR_DISPATCH` to every operator a trait names, and to the
  three things a struct instance's dispatch adds beneath the operand
  (2026-09-24), moves neither count (1096 and 340 before and after): no
  bundled body puts an arithmetic operator over two places of one
  parameter-typed type, and none of the bundled `__eq__` declarations
  consumes, converts, or leaves `!=` to `__eq__`. A user struct whose method
  adds two places of a type built over its parameter, one whose instance
  serves `!=` through `__eq__`, one whose dunder takes its operand by value,
  and one whose operand converts into the declared parameter type all derive
  for two instances (`template_method_operator_dispatch.mojo` and the inline
  sources beside it in `tests/compiler_test.rs`), and so does a method
  returning `a + b` under arithmetic bounds.
  An operation trait other than `Comparable`/`Equatable` admits only a
  numeric scalar (`conforms_to` answers `is_numeric_like`/`is_integer_like`
  for `Addable` and its family, never a struct's declared conformance), so a
  bare parameter bounded by one substitutes to a scalar in every instance;
  the struct arm of the recipe is reached through an operand whose type is
  *built over* the parameter instead (`Bag[T] + Bag[T]`).
- Hello World mints no per-instantiation clones at all. Its generated bodies
  are members of structs specialized whole and per-call clones, from
  concrete-only templates.
- The census (`template_census.*`) says which table recipes come next: of
  the 244 clone bodies still inferred, 179 are capturable already, and the
  largest blockers left are a binding the grammar refuses (16, a local of a
  parameter type), `ExplicitDestroyCalls` (13, sole blocker of 8),
  `EraseCompileTimeArgument` (8), and `PointerOriginCast` (7);
  `BorrowViewResult` (6, the `Dict.keys`/`values`/`__iter__` family) has a
  recipe since. Its
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
  initializer, a raised string or a raising call, the method's own binders, a `for` loop, a string
  other than the `_mojito_abort` message or a sink's argument, a call passing
  a `ref` local or a reference call's result, and a local whose type is built
  over a parameter
  (`var result = List[Self.T]()` in
  `List.__mul__`, `List.__getitem__(slice)`, and the owned `__iter__`s).
- An annotated `var` whose annotation is left to inference or whose value
  converts to a view that borrows its source
  (`var span: Span[Self.T, _] = self.items`).
- A construction with an `ImmOrigin` or a value compile-time argument, one
  whose constructor has binders of its own or binds a variadic parameter,
  one whose argument is a `ref` local bound to anything but a fieldwise
  struct's reference field or is a reference call's result, and one that
  wraps a sibling's immutable view (`ImmOrigin(origin_of(self))`). A method
  called on a sibling's view result itself (`self.entries().size()`). A
  pointer's own place provenance inside a retained type is still refused;
  only a struct's origin arguments are kept by template owner.
- A reference used as an argument, as the receiver of a method its bare
  parameter type promises through a bound, or as the value of a store
  (`self.items[i] = self.items[j]`, which the checker also judges by the
  element's type), and one yielded by a callee on anything but a field of
  `self`. A struct with an origin or a value parameter (`Span`, `Array`)
  derives no method at all.
- An augmented element store through a value getter and a setter
  (`self.table[i] += 1` where `__getitem__` returns a value): the setter binds
  the computed value at a synthesized source span, `(source, DUMMY_SPAN)`,
  shared by every such store in the module, which is no occurrence of the
  body, and records temporaries there. A struct element's `+=` through its
  `__iadd__`, and a store on `self` itself (`Dict.update`'s
  `self[k.copy()] = v.copy()`) likewise.
- An instance whose argument may carry a loan, which includes every struct
  with a field of a parameter type (`DictEntry[K, V, H]`).
- Per-call method clones and members of a struct specialized whole, which
  leave no trace.
- A local of a parameter type, a `for` loop, or a non-scalar result in a
  surviving trait-bound `def` template. Scalar locals and runtime `if` and
  `while` are covered.
- Any call that records a conversion, an adjustment, or an origin, and a
  transfer that does not vanish: a call-through residue that names a
  compile-time callable (`Tuple.deinit_with[elt_handler]`) or whose argument
  carries an origin, a named callable's own effects behind a residue, a
  function value's baked effects, a destination a captured binding names,
  and an instance whose values may carry a loan. The
  `CallableCaptureAccesses` adjustment is a concrete caller's fact: a
  `capturing[_]` parameter's environment stays open in every clone, so no
  template records it.
- An operator with an operand that is not a place: a literal, a call's
  result, or a nested operator. A place records nothing in either check,
  while a literal records a materialization or a conversion and a temporary
  records the facts the operator path does not register at all.
- A bound dispatch whose instance witness overloads the requirement with
  two members of one arity, or bakes a binder of a generic struct's witness,
  and a `var self` requirement called on a place rather than a `^` transfer
  (an implicit copy). A struct's reflective `__hash__` default derives: the
  trait-default expansion declares it before any body is checked. A
  `hasher.update(x)` on a concrete hasher is a Mojito-only spelling (the
  pin's `update` takes bytes), so no fixture exercises it. `Dict.__hash__` constructs its `H2()` (`ConstructTypeParam`) and
  `Optional.__hash__` a `UInt8` tag (`SimdConstructions`), so both keep the
  clone check for those reasons.
- A folded value parameter, or a loop variable surviving into an instance
  anywhere but as a pack element's index.
- A local declared inside a `comptime for`.
- A pack forwarded whole (`print(*a)`), a pack-keyed struct's methods (a
  validated method has no class, and a whole-struct clone leaves no trace),
  `DType` and vector parameters, reflection, struct-valued parameters, and
  anything a validation run that ended without a verdict reached.

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
