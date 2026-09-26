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
| Declaration-level trace | `crates/mojito-comptime/src/comptime.rs:DefInstanceTrace` (`specialize.rs:generate_def_spec`) and `MethodInstanceTrace` (`generate_instance_clones`, `specialize.rs:per_call_method_clones`) |
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
  (`UnkeyedFact`): declaration types and effects, generic parameters,
  transferred origins, deletable declarations. The one exception is the
  entries a nested `def` statement of the body keys by its own identity,
  which its recipe carries (`unkeyed_growth`, `TemplateNestedDef`): the
  baseline counts those entries too, so any other growth still refuses.
- A hash leaf the body's check demanded is kept as a type, not refused
  (`hash_leaves`). Capture reads it from the checker's append-only demand
  log rather than from the deduplicated leaf store, so a leaf an earlier body
  recorded first still counts. Every kept leaf is closed, so an instance
  records each again at installation, beside the leaves its bound builtins
  and dispatches record at its own types. A symbolic leaf still refuses the
  body (`UnkeyedFact`: symbolic hash leaf types).
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
adjustment, kept apart from the other adjustments), interior references,
copyable reference-result reads, iteration protocols (a
`TemplateIteration`, from which an instance selects the protocol again), and
comprehension binders (a `TemplateComprehensionBinding`, which an instance
declares again from its clause's protocol), `with` desugars (a
`WithForm`, from which an instance builds its own desugar), and nested `def`
capture lists (inside a `TemplateNestedDef`, beside the declaration's
signature facts, each capture by template owner).

A replayed transfer is kept in template-local terms too: each call transfer
(`call_transfers`), the origins it merged into a binding's bookkeeping
(`transferred_origins`), the effect the body's own frame then publishes
(`transfer_effects`), and the callee summary read (`transfer_reads`), every
source beside the template's type of the binding it is rooted at, since a
binding whose type may carry loans has its own place as its origin and a
plain-data one has none. Two things are recorded and kept as a fact about the
body rather than as entries. An operator over parameter-typed
operands records nothing in the template, so the grammar names it (`operators`)
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
call's `BorrowViewResult` loan names nothing an instance changes. A callee
whose declared return origin projects an owned interior of its receiver
(`String.strip` over `origin_of(self)._get_owned_interior["bytes"]`) records
the projection's tags at the call (`ViewResultInteriors`), and each result
slot it fixes as immutable as an immutable binder
(`ConstructionImmutableBinders`). Both are the callee's declaration, which a
nominal receiver selects alike under every instance: the tags are installed
as they stand, and the binders are derived again from the kept
`call_result_origins`, so capture refuses only a call whose binders differ
from those. A struct application's origin arguments are kept unbound too,
since recording one erases them.

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
  per-call clone (`echo$y3:Int$y6:String` for `b.echo[String]` on
  `Box[Int]`) joins the same list and records the same trace, whose type
  bindings name the struct's binders and then the method's own, with the
  method's folded values and expanded packs beside them. The checker
  substitutes the method's own binders from the trace and the struct's from
  the receiver. A clone minted into a struct specialized whole, or into the
  CTFE subprogram, is not traced.
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
- **Synthesized occurrences.** A node the checker synthesizes from a
  statement — today the `with` desugar's — takes `SyntaxId::derived(parent,
  ordinal)`: one identity per statement and position, so every rebuild of one
  statement's desugar names the same nodes. `SyntaxOrigins::origin` traces
  such a node through its parent (`derived(origin(parent), ordinal)`), so an
  instance's synthesized node traces to the template's as a copied source
  node does.

An `OccurrenceId` is that pre-rekey identity plus a copy number. A template's
occurrences are all copy zero. An unrolled `comptime for` copies one template
occurrence once per iteration, and the copies are numbered in pre-order.
`CheckedBodyFacts::selected` lays a template's facts out over an instance's
occurrences: a dropped occurrence (an untaken arm, a zero-trip loop) takes its
facts, requests, and effect reads with it, and a copied one carries them once
per copy.

An instance occurrence with no template occurrence behind it refuses the
derivation. A folded value parameter or `comptime for` variable is not one:
the literal the elaborator writes keeps the identifier's identity and takes a
literal's facts rather than the name's (`folded_literals`,
`FoldedLiteral`). Those are its own type (`IntLiteral`, or `Bool`); its
`MaterializeLiteral` to the template's recorded `Int`, which an `Int`
instance value fits exactly; and, where the template lent the name to a read
parameter or printed it, the temporary the call reads. What a `var` or an
assignment records at its value — the binding's type, its deletability, the
store's invalidations — is the statement's and survives the fold. The
grammar admits such a name only where those facts are the literal's: never
as a prefix operand, nor beside a literal-typed or folded operand, since an
operator over two literals folds to a literal the template never typed. Where
a literal indexes a pack it says which element the copy is — the template's
`Ts[i]` at that occurrence is replaced under `i` bound to the literal and the
pack bound to its element list (`substitute_packs`), and folds to the
element.

## Certificate classes

Every class shares a declaration shape: a module-level `def`, plain type
parameters (plus scalar `Bool`/`Int` value parameters for a keyed body),
immutable regular runtime parameters, a concrete scalar result (a runtime
body may return a whole value of any type, `FunctionBody` below), and no
`raises`, captures, or decorators. An operator is admitted only over operands
whose recorded types are closed scalars, so no operator dispatches through a
bound. `BodyShape` is the grammar; `template_certificate` is the argument.

| Class | Body | Why an instance needs no inference |
|---|---|---|
| `ClosedScalarBody` | `return`s of closed scalar expressions | The body names nothing, so every fact is closed and inherited unchanged. |
| `FixedCalls` | plus direct calls of module-scope functions, passing literals, parameters, and further such calls | Selection is retained. No copy or adjustment was recorded, so every argument either matched its parameter exactly and still does after substitution, or converts through an `@implicit` constructor the instance selects again (obligation 20), which never re-ranks the callee. Borrows depend on slots and conventions, not types. The callee's effect summaries were empty and are re-read. |
| `BoundedOperations` | plus the built-in `len` over a parameter whose bound promises a length | The bound proved the call. The instance owes the witness, which `len_result_for_type` finds, and takes the read-in-place fact `infer_len` adds for a nominal struct. |
| `MethodScalarBody` | a method with a plain read `self` and no binders of its own, on a struct of plain type parameters: `return`s over closed scalars, parameters, reads of `self`'s scalar fields, the built-in `len` over a field, and argument-free method calls on `self` or a field with a trivial contract | A field read has the field's declared type under the struct's arguments in a template and a clone alike. The generated-declaration leniency a clone's name switches on bears on origin-bearing return annotations only, and the result is a scalar. A trivial call can change per instance only in its target. |
| `ScalarBranches` | source-validated bodies: `comptime if` arms, scalar `comptime for` loops, scalar locals and assignments, erased `rebind`s, and a loop variable or scalar value parameter read as a runtime value | Every arm was checked once. The instance keeps the occurrences the elaborator selected, and a folded name's literal takes a literal's facts. |
| `PackElements` | a source-validated body keyed on a type pack (`*Ts`), collected by one variadic parameter: `comptime for` over the pack's indices, each element read as `pack[i]` into a `print` statement, beside what `ScalarBranches` admits; the result may be `None` | The element was checked once at the dependent `Ts[i]` through the pack's bound, and recorded as a place read where it lies. Each unrolled copy carries the folded index, so the instance fixes the element per copy; `print` selects no callee and records at an argument only what its syntax decides, and the instance proves the fixed element `Writable` again (`realize_print_call`). A pack forwarded whole and a local inside the loop stay out. |
| `MethodBody(features)` | a method beyond `MethodScalarBody`; see below | One argument per feature. |
| `FunctionBody(features)` | a runtime `def` body beyond the scalar classes: a whole value of any type copied or moved between a parameter, a local, a direct call's by-value argument, and the result (`OPAQUE_MOVES`, `VALUE_ARGUMENTS`), a runtime `for` over a parameter or a local (`ITERATION`), and a condition tested through `__bool__` (`TRUTHINESS`), beside `STATEMENTS`; `template_facts.rs:FUNCTION_FEATURES` is the allowlist | Each argument is `MethodBody`'s for the same feature, on a body with no receiver. A direct call's whole-value argument bound the callee's parameter exactly with the function's parameter symbolic, and the parameter's type lives in the callee's own binder scope, so the substituted application binds it exactly too and the template's selection stands; the copy or the transfer is owed again per instance, and every instance argument is plain data (obligation 12). |

A body source validation did not produce (every class but `ScalarBranches`)
may also hold runtime statements over closed scalars: scalar locals and
assignments, `if`/`elif`/`else`, `while`, `break`, `continue`, a bare
`return`, and a discarded call or `_ =` value. A runtime statement is checked
once whatever runs it, so none drops or copies an occurrence. A condition's
recorded type must be exactly `Bool`, which `expect_bool` accepts without a
truthiness fact, unless the body holds `TRUTHINESS`. A keyed body keeps its own rules: a local is never declared
inside a `comptime for`, where an unrolled body would need one binding per
copy, and a folded name is read only where its literal records what the
name's occurrence would.

### `MethodBody`

The declaration may have a `mut`, `var`, `deinit`, or `ref` receiver (with
an origin naming one of the method's own origin binders, `ref [o] self`), the
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
is judged per instantiation and stays a clone check. A receiver origin
naming one of those binders is admitted the same way. A struct's own origin
or scalar value parameter is never cloned whole (`generate_instance_clones`
keeps it on the erased path), so its methods are certified for reuse across
passes only: a `Self.rows` read is a runtime read of the reified value, and a
pointer field whose provenance is the struct's origin names no checker-local
place. A receiver origin naming anything else, the copy and move
initializers, and any other binder stay outside. A `where`
clause is the declaration's constraint: `generate_instance_clones` mints a
clone only where it evaluates true, a trace exists only for a minted clone, and
a clone's signature no longer states it.

`MethodFeatures` names what the body holds. The features are independent, so
they are a set, not a ladder.

| Feature | Body | Why an instance needs no inference |
|---|---|---|
| `STATEMENTS` | a receiver other than a plain read `self`, or the runtime statements above, plus a store to a closed scalar field of a `mut`/`var` `self` or of a `var` local holding a whole value (`result.size -= 1` after `var result = self`) | A stored field is a closed scalar, so the store is a plain scalar write and never an in-place operator of the field's type. Invalidations name `self` and locals by template owner. |
| `OPAQUE_MOVES` | a whole value of any type moved (`^`) or copied between a parameter, a local, a field of `self` or of a `var` local, and the result, including `self` itself copied into a local (`var result = self`), never moved out of it | The value is never an operand, a receiver, a condition, or an argument, so nothing dispatches on its type. A store or a result has the value's own recorded type, which stays equal under substitution, so neither check converts. What a clone check still decides from the type is owed per instance (obligations 3 and 10 to 12 below). |
| `POINTER_SLOTS` | over a pointer field of `self`, or of a `var` local copied from it, with no tracked provenance, or whose provenance is the struct's own origin parameter (`Span._data`): `unsafe_offset(scalar)`, `unsafe_take_pointee()`, `unsafe_deinit_pointee()`, `free`, the slot `pointer[scalar]` as a store target or a copied place, and an offset stored back to such a field (`result._data = result._data.unsafe_offset(start)`) | Such a pointer holds no loan, names no place, and is a pointer under every instance, so its methods are the built-in ones (`infer_pointer_method`), which select no callee and record an adjustment naming at most the pointee. Every other judgment there only produces an error, and the template's is at least as strict. |
| `SIBLING_CALLS` | a method call on `self`, a field of it, or a `var` local, passing closed scalars, whose contract is a `closed_method_contract`; its result may be a view over the receiver (`_DictKeyIter(self.items())`) | Obligation 7 below. Arguments are closed scalars in both checks, so nothing about them depends on the instance. A view result's `BorrowViewResult` loan and the origins its contract resolves are the callee's declared `origin_of(self)` return over the call's own receiver, which no instance changes; a constructor the view is handed to matches its field or parameter but for those origin arguments. |
| `REFERENCE_RESULT` | a method that returns a reference: every `return` hands out a field of `self`, a pointer slot, or a reference call's result, of exactly the declared referent type | The `return` keeps its value as a handle because the declaration returns a reference (`ReferenceValueUses`, written from the declaration and the statement's syntax alone) and demands neither a copy nor a move. Whether the place lies within the declared origin is judged on its path, the signature, and the receiver's constructor, none of which an instance changes; the loan-escape check reads what obligation 12 already rules out. The signature is checked per clone before the body, derived or not. Any other handle in the body refuses it. |
| `REFERENCE_CALLS` | a subscript or a named accessor on `self`, a field of it, or a `var` local (`result.value()` in `List.index`), passing closed scalars, whose contract is a `closed_reference_contract`, either returned as above or read by value into the result or a `var` | Obligation 13 below. The call is never a receiver, an operand, a condition, or an argument, each of which records a borrow of its own. A by-value read is admitted only where the template marked the read copyable. An iterator's `__next__` marks its read by another rule and stays out. A scalar element of a tuple-typed `var` local at a literal index (`bounds[0]`) is such a call too, on the generated `Tuple`'s accessor for that position (`__getitem_param__$k`); the template records it only where its own `Tuple` was already declared and closed, and records nothing but types otherwise, so an instance records the call afresh on its own `Tuple` (`realize_tuple_elements`). The built-in `slice.indices(n)` on a slice parameter or local selects nothing and records only closed types. |
| `REFERENCE_LOCALS` | `ref name = place`, where the place is `self`, a field of it, a parameter, a `var` local, or a reference call; then a field read through the binding, the binding copied out whole, a scalar operand, `len` over it, a scalar store through it, or the binding forwarded as the method's own reference result | The declaration decides mutability from the value's reference and the binding it names, re-stamps an origin computed upstream, and runs no check of its own. Its type is a reference whose origin names a binding, so a bundle keeps it by template owner, as it keeps a call's, and an instance substitutes the referent. Every use records the referent. A copy out owes obligation 3, and a copyable read obligation 13. |
| `REFERENCE_RECEIVERS` | a field read or a closed method call through a reference call's result or a `ref` local whose referent is a struct | A call borrows such a receiver (`BorrowedReferenceReceivers`) because of what the receiver is: a reference result, a `ref` binding, a `ref` field. None of those tests reads the referent. The callee is realized as in obligation 7, from the receiver's own substituted type; a receiver whose type was already closed in the template (`List[Pair]`) selected its clone there. A method of a bare parameter dispatches through the bound and stays out. |
| `SUBSCRIPT_STORES` | a store through a subscript of a writable `self` or of one of its fields: a closed scalar field of the element a reference getter yields (`self.entries[i].hits = 0`, `+= 1`); an element a declared setter takes, a closed scalar or a whole value of the setter's own parameter type (`self.counts[i] = n`, `self.items[i] = value^`, `self.entries[i] = Entry[Self.T](v^)`, `self[k] = v^`); a closed scalar element stored whole or augmented through the mutable reference its getter yields (`self.counts[i] += 1`, `self.grid[i] = n` on a struct with no setter) or augmented through a value getter and a setter (`self.table[i] += 1`, `self.box[i] += 1` with `box: Box[Self.T]`, `self[i] += 2`); or a struct element stored augmented through its in-place dunder, of a closed type (`self.counters[i] += 3`) or of a bare parameter type whose bound requires the dunder (`self.items[i] += x`, `items: List[Self.T]`) | A subscript that is a place records its index shape and whether the setter takes the value by keyword (`SubscriptDescriptors`). The first is the syntax, one plain index; the second is the setter's declaration. The getter is a `REFERENCE_CALLS` call and the setter a `SIBLING_CALLS` one, recorded at the subscript and realized by the setter's name; its index and value are the call's arguments (`VALUE_ARGUMENTS`), so a whole value binds a parameter of exactly its own type and owes what it owes at any call (obligations 3, 10, and 14). A store through a mutable reference records no second contract: the checker writes the computed value back through the getter's reference, and its `AugmentedSubscript` record is that getter beside the element's type, kept apart from the adjustment table (`augmented_subscripts`, as `reference_results` are) and rebuilt for an instance from the getter it realized at the site. A store through a value getter and a setter records the setter at the subscript, and the checker keys the computed value it binds there too, since that value is no expression; its `AugmentedSubscript` record keeps the value getter beside the setter in `augmented_subscripts`, and an instance realizes it on its own subscripted value exactly as it realizes the setter (`realize_element_getters`, through the `realize_method_contract` half of `realize_method_call`). A struct element's `+=` selects its `__iadd__` on a synthesized receiver whose facts and identity the checker drops with its scope; the dunder is kept beside the store likewise. Over a closed element type it stands as recorded; over a bare parameter it is the bound's abstract requirement, whose witness an instance selects on its own element type from the types alone (`realize_element_dunders`, `realize_embedded_dispatch` over `bound_witness`), with an operand of the element's own type bound by value. A whole element read from the same list (`self.items[i] = self.items[j]`) derives like any whole value the setter takes. |
| `PLACE_ARGUMENTS` | a local, a parameter, or a field of `self` handed to a `mut` or bare `ref` parameter of a method call | The callee's declared convention decides that the call keeps the caller's place (`CallPlaceUses`), and the generations a `mut` argument invalidates lie below the argument's own binding, kept by template owner. A kept place is neither copied, moved, nor converted, and its recorded type equals the parameter's. Whether two arguments conflict is judged on their places and conventions. A field of `self` is kept only beside a receiver the call reads. A parameter of a struct parameter's type is admitted only on a call of `self`'s own method, where callee and caller share one binder scope, so the instance substitutes it in the contract and in `CallParameters`. |
| `ORIGIN_PARAMETERS` | a `ref` parameter with an origin clause, the method's origin binders, a receiver origin naming one of them, and the parameter forwarded as the method's own reference result | The clause lives in the signature, which is checked per clone. The body's facts for such a parameter are a bare `ref` one's: its binding, its type, and a copy where it is read by value. |
| `VALUE_ARGUMENTS` | a whole value of any type handed to a by-value parameter of a method call: a `^` transfer, a sibling call's result, a place the template copied, or a named place a read parameter takes where it lies | The argument's recorded type equals the parameter's, before and after substitution, or an `@implicit` constructor converts it to the parameter's, and the instance selects that constructor again (obligation 20). What the call records for it is decided without its type: a read parameter borrows a named place and reads a temporary by the argument's syntax and the callee's conventions, and a `var` parameter takes a transfer or a temporary as it stands. A copied place owes obligation 3 and a transfer obligation 10, as elsewhere. A `ref` local and a reference call's result stay out, since each records a borrow of its own. A callee with a parameter of a struct parameter's type may belong to a field of another struct: it has no binders of its own, so its parameter types were recorded at the receiver's arguments, in the caller's binder scope, and the instance substitutes them in the contract and in `CallParameters` alike. An overloaded family whose members declare parameters of parameter types is admitted when every argument's type is exactly its parameter's: no member outranks an exact match, and a family an instance collapses finds no single clone (`method_clone_target`). |
| `REPLAYED_TRANSFERS` | a call whose callee stores an argument outward (`self.items.append(value^)`, `self.items.append(held^)` from a local), so the template replays a transfer summary at it | A transfer moves the loans its source carries. The template's parameter may carry one (`type_may_carry_loans` is true for a symbolic parameter), so its check records a call transfer whose source is the parameter's own place, merges that origin into the destination's bookkeeping, and publishes an effect of its own; the bundle keeps all three by template owner, each source with its binding's type (`body_transfers`). The template's own reuse installs them again on its own bindings; an instance replays each transfer against its realized callee's summary and keeps a source only while its binding may still carry a loan (`realize_transfers`), so a plain-data instance records nothing at the call and publishes nothing, which is what its clone check records. Obligation 14 below. A call-through residue, a function value's baked effects, a destination a captured binding names, and an effect whose source is not a single parameter or the receiver are not replays and still refuse. |
| `OPERATOR_DISPATCH` | Every operator a trait names — `==`, `!=`, `<`, `<=`, `>`, `>=`, and the arithmetic, bitwise, and shift ones — over two operands of one type that mentions a struct parameter, each a place, a call result, or another such operator, or over such an operand and a right-hand literal beside a struct built over the parameter | The template proves the operator through the bound, or dispatches the dunder of the struct built over the parameter, and records nothing at the operator; `infer_infix` decides a struct operand's dunder from the operand types alone (`struct_infix_dispatch`). Obligation 15 below. Neither check records anything at a temporary operand: a place is read where it lies, and a call result or an operator's value is moved into the dunder or dropped after it (`check_consuming_as` copies only a place). What the instance's dispatch adds beneath the operand is the copy of a consumed place, the conversion of an adapted one, and `NegatedEquality`; a literal's conversion into the dunder's parameter type is the template's, selected again for the instance. A literal left operand is refused: its dispatch is the reflected dunder, which the template records at the operator. An arithmetic operator's result is the operand's own type rather than `Bool`, so it is a temporary of that type wherever the body puts it (`BodyShape::operator_value`, beside a call result and a construction). |
| `BOUND_DISPATCH` | a method call on a place of a bare parameter type, or on a named place's `^` transfer for a `var self` or `deinit self` requirement, proved through a bound: `place.copy()`, `item.__hash__(hasher)`, `item.write_to(writer)`, `item.bump(2)` on a `mut self` requirement, or any requirement whose arguments are closed scalars or named places handed to a bounded `mut`/`ref` parameter, each of a bare parameter type or of a closed type the template proved against the bound | The template records the abstract dispatch (`__trait_dispatch.__hash__$ov$…`), or for `write_to` the inverted write, and nothing the receiver's type decides: a kept argument's place use and generation refresh follow from the requirement's convention, which the witness shares. `infer_method_call` decides a concrete receiver's witness from its type alone, and `bound_witness` repeats that decision from the types. Obligation 16 below. The call through the bound read the summaries of every conformer's method of that name, one key per conformer; each was empty, and the instance re-reads only its own target's. |
| `BOUND_BUILTINS` | `hasher.update(x)`, `hasher._update_with_simd(x)`, or `writer.write(x…)` on a parameter bounded by `Hasher` or `Writer`, whose arguments are closed scalars, string literals, named whole values, `ref` locals, fields read through a reference, pointer slots, or reference calls | The builtin selects no callee and records at an argument only what its syntax decides: a borrow of a named place or a reference result, an unconsumed temporary, a closed literal's materialization. The argument's type it proved through the bound, and the instance proves it again at its own type (obligation 17), which for a hashed value is where the hash leaf is recorded. |
| `BOUND_BINDERS` | the method's own type binders, each a plain trait-bounded type (`[H: Hasher]`); a construction of one as a whole value (`var inner = H()`), and `inner^.finish()` on the `^` transfer of a place of one | A clone keeps such a binder, bound symbolically as the template binds it: no clone is minted per hasher, the VM reifies the binding at run time, and no retained fact substitutes it. The instance's substitution binds the struct's parameters alone (`instance_substitution`), as it does beside an origin binder. The construction's `ConstructTypeParam` names the binder by spelling, so the grammar reads the recorded type's declaration to refuse a struct binder, and the instance keeps the adjustment only while its substitution leaves that type alone (`kept_binder_construction`). `finish` is a checker builtin on such a receiver, yielding `UInt64` under every instance. |
| `CONSTRUCTIONS` | a construction of a declared struct whose compile-time arguments are types, as a whole value: `copy:` of a named place, or arguments that are closed scalars, whole values, a place lent to a hand-written constructor's `ref` parameter (`REFERENCE_ARGUMENTS`), or — for a fieldwise struct's reference field — a `ref` local | A construction records no contract: `infer_construction` selects an `__init__` from the argument types, retargets it to the instance's constructor clone, and reaches the struct application. Its one unconditional record, `ConstructionImmutableBinders`, is empty when no compile-time argument is an `ImmOrigin` cast, which the type-only arguments guarantee, and installation writes the empty entry again. What a constructor records at an argument is decided by the argument's syntax and the constructor's conventions (a copy owes obligation 3, a transfer obligation 10, a literal materializes to a closed type); the selected member binds each argument exactly, so no member can outrank it under any instance (obligation 18). A constructed type that names the receiver in an origin argument (`_ListIter[T, origin_of(self)]`) is kept with the slot unbound and the origin by template owner (`typed_origins`), and the `return`'s re-resolution of the annotation's `origin_of(self)` is repeated once by the instance rather than recorded. |
| `CALLABLE_PARAMETERS` | a call through a parameter declared with a `def(...)` type, passing closed scalars or whole values, as a statement; and such a parameter forwarded by value to a sibling call | The call records the parameter's own contract symbol and parameters, which the instance takes from its own binding of the parameter, and puts a call-through residue on the body's frame that names the parameter's slot and each argument's signature place. A forwarded parameter reads the callee's residue and composes one. Neither names a type: the instance republishes the template's residue and owes that its realized callee publishes the one the template read. Obligation 19 below. |
| `STRING_BUILTINS` | `_unqualified_type_name[T]()`, and `repr(value)` over an argument a checker builtin reads where it lies | Neither selects a callee. The reflection call records one type's spelling, which `derive_adjustment` re-renders from the substituted type (`TypeName` carries the type beside the text) and refuses while that type is still symbolic. `repr` proves its argument `Writable` — which the instance proves again at its own type — reads it where it lies as a bounded sink's argument is read, and wraps its compile-time string result as the nominal `String`: one conversion, whose literal constructor is the same under every instance (obligation 20).
| `REFERENCE_ARGUMENTS` | a `ref` local, a field reached through a reference, or a reference call's result handed to a method call's read, `var`, or `mut` parameter, or lent to a hand-written constructor's `ref` parameter | What the call records there is decided by what the argument is, as for a receiver reached through a reference: a read parameter borrows the place the argument names (`BorrowedReadCallPlaces`, by syntax), a `mut` parameter keeps it (`CallPlaceUses`), a `var` parameter copies it where the template recorded the copy (obligation 3), and a field read keeps its base as a handle. A reference call records its own result and interior, and its copyable-read mark is taken again at the instance's referent (obligation 13). A constructor's `BorrowRefArguments` names the lending positions, which are the selected constructor's declaration, and each loan's mutability, which is the place's; `derive_adjustment` carries it verbatim. One whose loan materializes a temporary names a binding of one run and refuses. |
| `RAISES` | a `raises` declaration, bare or typed, and a `raise` whose operand is a `CONSTRUCTIONS` construction or `Error("…")` of a string literal | A `raise` records nothing of its own. `require_error` asks two things of the operand's type: whether it is a string, which a constructed struct's name and the builtin `Error` settle whatever the instance, and whether it equals the declared error type, which it does under every substitution if it does symbolically, both types being written over the same parameters. The declared error type lives in the signature, checked per clone. A raised string literal, a raising call in the body (`effects_closed`), and a module-level `def` that raises stay outside. |
| `CONSUMING_CALLS` | a method call on the `^` transfer of a named place the body owns — a `var` local, a `var` parameter, or a field of a consumed `self` — whose callee on the receiver's nominal struct takes it as `var self` or as a named `deinit self` destructor (`entry^.reap_value()`, `self._alloc^.unsafe_leak()`, `result^._take_items()`), passing arguments `VALUE_ARGUMENTS` admits | The move is recorded at the transfer, which owes obligation 10, and not in the contract, so the instance changes the contract only in its target and its substituted types (`consuming_nominal_contract`). The call's one other record, the explicit-destroy mark (`ExplicitDestroyCalls`), says the receiver's struct declares the method with `deinit self`: a struct's destructors are keyed by name on its declaration (`explicit_destructors`), which its arguments do not change, so an instance inherits the mark. Through a bound, a `deinit self` requirement on a `^` receiver is a `BOUND_DISPATCH` call, and its instance's witness is asked again: a struct that implements the requirement with a named destructor takes the mark the template's parameter-typed receiver could not have (`realize_bound_dispatch`). A defaulted argument, evaluated in the callee's scope, stays outside. |
| `COPIED_RECEIVERS` | a method call whose callee consumes its receiver, on a named place the call copies first rather than on a `^` transfer: a parameter, a `var` local, or a field of `self`, of a parameter, or of a local (`slice.start.or_else(0)`, `self.limit.or_else(n)`), on a nominal struct as `CONSUMING_CALLS` admits it or through a bound to a `var self` requirement (`coin.spend()`) | `infer_method_call` copies such a place whatever its type, where the type is implicitly copyable, and rejects the program otherwise; the mark it records at the call (`ImplicitlyCopiedConsumingReceivers`) is decided by the receiver's syntax and the callee's convention, so an instance inherits it and owes the copy at its own type (obligation 3). The contract is a consuming call's, and nothing moves out of the place. Through a bound, the instance's witness must itself take `var self` or `deinit self`, or the derivation refuses (`realize_bound_dispatch`). |
| `DIRECT_CALLS` | a direct call of a module-scope function that is not generic and takes only closed scalars by value (`check_slice_bounds(start, end, self.size)`, or a member of an overload set such as `range(n)`, which may also be a `for` or comprehension iterable) | The call selects the same declaration, or the same overload member, under every instance: an overload set ranks only the argument types, and those are closed. It binds its arguments at types no substitution changes; the instance realizes it as a `FixedCalls` body's direct call (obligation 5), re-reading the callee's empty effect summaries. |
| `ITERATION` | a runtime `for` with no `else`, over `self`, a field of it, a parameter, a local, the `^` transfer of a place the body owns, or a sibling call's result; the loop variable is a local whose kind — a handle, a scalar, or a whole value — follows its recorded binding type | The protocol is selected from the iterable's type and resolved against the place the loop borrows and that binding's mutability, which are the loop's syntax and the declaration's. The template keeps those inputs (`TemplateIteration`), and the instance selects again from its substituted type (obligation 21). |
| `SIMD_CONSTRUCTIONS` | a `SIMD`, `Scalar`, or scalar-alias construction from closed scalars (`UInt8(1)`, `SIMD[DType.uint8, 4](1, 2, 3, 4)`) whose dimensions the template recorded, handed to a checker builtin (`hasher._update_with_simd(UInt8(1))`), to a by-value parameter, or bound to a `var` local | Inference records a construction's dtype and width (`SimdConstructions`) only when both are closed, so a recorded entry is the same under every instance and is installed as it stands. The call selects no callee and converts nothing. A construction whose dtype or width names a parameter records nothing in the template; no enabled class reaches one today, since a `DType` or `Int` value binder is outside every class. |
| `TRUTHINESS` | an `if` or `while` condition that reads a parameter, a local, or a field of `self` whole and tests it through `__bool__` (`if self._value:` in `OptionalReg.or_else`) | `expect_bool` marks such a condition (`TruthinessConditions`) from its type alone: a `Bool` is read as it stands, and a width-one bool lane or a struct whose `__bool__` returns `Bool` converts through `Bool(x)`. The read itself records only the place's type and binding, neither a copy nor a conversion. Obligation 22 below. |
| `TUPLE_UNPACKS` | a tuple unpacked from a parameter, a `var` local, a field of `self`, or a sibling call's result into `_` and `var` locals, declared by the statement (`var v, n = t`) or before it (`v, n = t`); a declared target is a scalar or a whole-value local by its recorded binding type | The statement records one plan at the value (`TupleUnpackPlans`): each element's type and, for a generated Tuple, the accessor that reads it and the reference a place accessor yields. The plan is a function of the value's type and the reference the unpacked place yields, which the template keeps (`TemplateTupleUnpack`) beside the named targets. Obligation 23 below. The checker judges no deletability at an unpacking's target, so realization judges none there either. |
| `PARAMETERIZED_CALLS` | a method called with explicit compile-time arguments, each a literal or a type (`self.scaler.scaled[3](x)`, `self.scaler.show[Int](x)`), on a receiver a sibling call admits whose type is a non-generic struct | The call records the selected method's declared compile-time parameters (`ParameterizedMethodCalls`), which are the callee's declaration and are installed as they stand, and a per-call clone request (`MethodInstantiation`). Once that clone exists the contract names it (`Scaler.scaled$i3`) and declares no parameters, and the instance keeps the target (`realize_method_call`); before it exists the contract still carries the declared parameters, which the grammar refuses. A request that substitution would change (`show[Self.T]`), or one keyed by a generic receiver, refuses. |
| `COMPREHENSIONS` | a list, set, or dict comprehension, as a whole value (returned, bound to a `var`, or the element of another comprehension), whose `for` clauses iterate what `ITERATION` admits, whose filters are runtime conditions, and whose produced key and value are closed scalars or whole values; each binder is a local scoped to the clauses after it and the produced elements, of the kind its recorded binding type makes it | Each clause records its protocol at its iterable as a loop does, and each binder's plan, type, and droppability (`ComprehensionBindings`) follow from that protocol and that type. The template keeps the binder's owner and the iterable whose protocol declares it (`TemplateComprehensionBinding`); obligation 24 below. The collection's `ConstructCollection` adjustment names the target struct's own insert method, which substitution keeps. |
| `COMPTIME_CONTROL` | a `comptime if` or `comptime for` in a method source validation checked, on an ordinary generic struct (`comptime if Self.T == Int`, `comptime for i in range(3)`), with a loop variable read as a runtime value and a local declared inside the loop | Every arm was checked once with the struct's parameters symbolic. The instance keeps the occurrences of the arms the elaborator selected, once per unrolled copy, and drops the rest with their facts (`CheckedBodyFacts::selected`); a folded loop variable takes a literal's facts (`folded_literals`) and a loop-local one binding per copy (`renumber_locals`), exactly as `ScalarBranches` argues. The clone's trace names its own first statement (`MethodInstanceTrace::clone_body`), since a body opening with a `comptime if` keeps the arm's statement, not the template's. A `rebind`-keyed method, whose equalities only an instance can discharge, stays outside. |
| `WITH_STATEMENTS` | a `with` statement, one item or several, each context a whole value, in any form the checker desugars: a plain `__exit__`, an error `__exit__` in a raising method, a consuming `__enter__` with or without `as`, or no `__exit__`; the body a block of the grammar | The checker desugars the statement into ordinary statements (`with_stmt.rs`), and the grammar judges that desugar in its place once the facts are captured: the manager a `var` local, its `__enter__` and `__exit__` sibling calls, the guarding `try`, the error handler's `raise` of its own binder, and the `_mojito_keep_alive` anchor, which selects nothing. The occurrence walk reads each desugar in its statement's place (`occurrences_over`). Which form the desugar takes is the manager struct's declared `__enter__` and `__exit__` members and the raising context, never a substituted type, so the template keeps the form (`WithForm`, from the `WithDesugars` table) and an instance builds its desugar again from its own syntax (`instance_with_desugars`, `with_stmt.rs:with_desugar`), which installation hands to the final splice. Obligation 25 below. |
| `NESTED_DEFS` | a nested `def` with no compile-time parameters, decorators, `where`, or `raises`, taking regular read parameters, whose recorded parameter and result types are closed scalars (or no result), with no capture list or an explicit one of `imm`, `mut`, and `var` entries each naming a local or a parameter of the method; its body judged in place, its parameters scalar locals and a `return` a closed scalar; called from the method with closed scalar arguments | The declaration's check records its signature under the statement's own identity — parameter, return, and callable types, its (empty) compile-time parameters, its effect, each parameter's deletability — and its capture list at the statement; the template keeps them at the statement's occurrence (`TemplateNestedDef`), and the instance writes them again under its own statement. A capture names a binding and roots its origins at one, and so does the callable's environment and each call's `CallableCaptureAccesses`: all are kept by template owner and rebound to the instance's own bindings, the environment re-canonicalized. The parameters are locals the statement declares after its name. A call selects the declaration the body introduced, under every instance. Obligation 26 below. |
| `STATIC_CALLS` | a static method of a non-generic struct called on its type with closed scalar arguments, spelled (`Color.pick(n)`) or through a leading-dot contextual root the expected type resolves (`.of(n)`, `return .red()`, `var c: Color = .of(n)`), as a scalar or a whole value | The receiver is a type, so the call records no contract, call parameters, or application: at most the overload member its closed arguments ranked, which no instance changes. A leading-dot root records the head of the expected struct type (`ContextualBases`), which HIR substitutes for the sentinel; an expected type that is a bare parameter refuses the form, so the head is the same under every instance and the instance inherits the entry. A static of a generic struct, which infers or binds the struct's own parameters, stays outside. |

The compiler-private trap `_mojito_abort("message")` is a statement of any
non-keyed body: the built-in types its literal and selects nothing. A
declaration of that name would record a binding and parameters at the call,
and such a call is not admitted.

A built-in scalar conversion of one closed value (`Int(key_hash)`, also
`UInt`, `Bool`, `Float64`) is an expression of any non-keyed body, and so is
one of a named place of a closed struct type (`Bool(result)` in
`List.index`), whose conversion dunder reads the place where it lies. It
selects no callee and records only closed types and a borrow its syntax
decides, which no instance changes (`BodyShape::scalar_conversion`).

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
   implicitly copyable at the instance's type, as `check_consuming` demands,
   and so must a receiver a consuming call copies, as `infer_method_call`
   demands.
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
12. **Plain-data arguments** (`MethodBody` and `FunctionBody`). Every instance argument
    carries no loan, holds no reference, and mentions no callable. A clone
    check decides outward-store transfer effects, view-result borrows, and
    closure escapes on those properties, and a template, whose parameter is
    symbolic, records none of them. `type_may_carry_loans` is conservative for
    a struct whose declared fields have parameter types, so
    `List[DictEntry[…]]` refuses today. One exception: a clone whose only
    binders are the origin binders the elaborator declared for a
    loan-carrying argument (`Bag[Span[Int, __clone_origin0]]`) takes that
    argument, whose loans are exactly those binders' and whose transfers
    obligation 14 replays. The template's `self` and `var` parameters carry
    loans symbolically already, so the clone check records the same
    transfers.
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
14. **Replayed transfers.** Each transfer the template replayed is replayed
    again for the instance (`realize_transfers`). The realized callee's
    summary must be the one the template read, or empty. A source is kept
    while the substituted type of the binding it is rooted at may still
    carry a loan, and vanishes where that type is closed and holds no loan,
    reference, or callable in its storage, its fields at their own arguments
    (`loan_free`): a plain-data binding has no origin, so the instance's own
    check records none for it. A transfer, a merged origin, or an effect
    whose every source vanished is not recorded; a call whose realized
    summary is empty must lose every source, and a read whose realized
    summary is empty is observed empty, as obligation 9 observes it. The
    template's own reuse substitutes nothing and keeps every source. The
    escape verdict `replay_transfer_effects` reaches is monotone in the
    sources, so an instance, whose sources are a subset of the template's,
    cannot fail where the template passed. `MOJITO_VERIFY_TEMPLATE_FACTS=1`
    checks the identity case, the plain-data instances
    (`assets/ok/template_method_transfer_replay.mojo`), and the loan-carrying
    ones (`assets/ok/loan_carrying_instance_clone.mojo`): a `var value:
    Span[Int, __clone_origin0]` parameter moved into `self` keeps its source,
    the parameter's own place, exactly as the clone's own check records it.
15. **Operators.** Each admitted operator is dispatched on the substituted
    operand type (`realize_operator`): a closed scalar records nothing, owing
    only that the primitive path has the operation and gives the type the
    template kept (`scalar_operator_result`); a nominal struct records the
    dunder target `struct_infix_dispatch` selects, when the dunder is
    overloaded or a per-instantiation clone, and the operand's application as
    a receiver would, and its result must still be the type the template
    kept. A dunder that consumes a place operand records its implicit copy,
    and owes that the instance's type is implicitly copyable, unless the
    template's own dispatch recorded it already; a temporary operand is moved
    and records nothing. One that converts its operand records the
    conversion, whose constructor obligation 20 selects, unless the template
    kept one there (a literal beside a struct built over the parameter), which
    the instance must convert too; `!=` served by `__eq__` records the `NegatedEquality`
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
    (`specialized_method_clone`, or `instance_call_method_clone` for a
    generic struct's witness, keyed by the instance and the call together)
    the call names that clone, as the clone check retargets to it. A
    witness the instance has no clone of names the template's method, its
    availability condition judged at the instance's arguments; beside the
    instance's other clones that is admitted only for a synthesized trait
    default (`Copyable.copy`, the reflective `__hash__`; `MethodSig::
    synthesized_default`), which the elaborator never clones. A `mut self` requirement keeps the receiver's
    place and generation refresh as the template recorded them, and a
    `var self` one is admitted only on a named place's `^` transfer, which
    records the move at the receiver itself. An inverted write whose receiver the
    instance makes a struct other than `String` becomes the struct's own
    `write_to` call the same way (`realize_inverted_writes`): the adjustment
    and the receiver's borrow go, and the writer becomes a kept place with
    its generation refresh. A witness of another shape, a source witness
    withheld from an instance that has other clones, and a type that is
    neither a built-in nor a declared struct refuse. Bound dispatches are realized
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
21. **Iteration protocols.** A loop's protocol is the `__iter__` chain and
    `__next__` selected from the iterable's type (named for the instance's
    clone family where one exists), the iterator's declared projection, and
    origins rooted at the place the loop borrows. Capture keeps what the
    selection is made from (`TemplateIteration`): the iteration and binding
    modes, the iterable's type, the source place by template owner (the
    attached borrowed origin without the projection), and the source
    binding's mutability, read back as the value that reproduces the
    recorded protocol. Capture rebuilds the template's own protocol from
    those inputs through the loop statement's own path
    (`loop_site_protocol`) and refuses unless it matches exactly, so the
    recipe cannot drift from `check_stmt`. The instance substitutes the
    iterable's type, which must stay the same nominal struct or a variadic
    pack, and selects again (`realize_iterations`); a raising `__iter__`, a
    missing protocol, and owned iteration's element bound refuse there.
    Installation resolves the protocol against the instance's own binding of
    the source (`install_iterations`). The checks a loop makes on an owned,
    non-`Deinitable` element (no early exit, no unhandled raise) are the
    template's at least as strictly: a bare parameter element is judged
    linear in the template.
22. **Truthiness conditions.** A condition's mark is `expect_bool`'s verdict
    on its type (`condition_truthiness`), which an instance asks again at
    its own recorded type for each condition the template marked: a
    condition that became `Bool` drops its mark, and one that is no longer
    boolable refuses. A condition the template read as a `Bool` stays one
    under every substitution, so only the template's marks are judged.
23. **Tuple unpackings.** An unpacking's plan is built from the value's type
    and, for a place, the reference it yields (`tuple_unpack_plan`, which the
    statement itself calls). Capture keeps those inputs, the reference by
    template owner, and rebuilds the template's own plan from them, refusing
    unless it matches exactly. The instance substitutes the value's type,
    which names the generated Tuple the clone check selects for a closed
    public one, and proves its plan builds (`realize_tuple_unpacks`): a
    temporary of a generated Tuple is read through value accessors, which
    exist only for implicitly copyable elements. Installation builds the
    plan against the instance's own binding of the place
    (`install_tuple_unpacks`). The statement's declared targets are locals
    of the body, numbered in declaration order beside the statement-bound
    ones (`renumber_locals`). Every retained type an instance substitutes
    names the generated Tuple for a closed public one
    (`canonicalize_public_tuple_types`), as the clone check's annotations
    do, so a `Tuple[Self.T, Int]` parameter reads as `Tuple$t2[…]` in both.
24. **Comprehension binders.** A binder's plan is its clause's protocol's
    binding, its type that plan's binding type, and its droppability
    `is_deinitable` of that type (`check_comprehension`). Capture proves all
    three against the recorded binding and keeps only the binder's owner and
    its clause's iterable (`captured_comprehension_bindings`). The instance
    selects the clause's protocol again (obligation 21), and installation
    declares the binder from it on the instance's own binding
    (`install_comprehension_bindings`). A binder is minted while the
    statement holding the comprehension is checked, before that statement's
    own binding, so where a body holds one its locals are numbered in the
    template's order rather than by pre-order (`renumber_locals`); only a
    body with no unrolled copies holds one. A filter over a linear binder is
    rejected by the template's check, and an instance's binder is droppable
    wherever the template's was.
25. **`with` desugars.** The instance's desugar is built from its own
    statement in the template's form, node for node: the check and the
    derivation share one builder, and every synthesized node's identity is
    derived from the statement's, so the instance's occurrences are the
    template's exactly (the derivation refuses a statement with no recorded
    form). The facts at those occurrences are the ordinary ones — the
    manager's binding, its construction, its sibling calls, the handler's
    binder — and are realized as elsewhere. A verifying inference rebuilds
    the same desugar, and `replace_body_facts` keeps it across its clear.
26. **Nested `def` declarations.** Each nested `def`'s signature is closed,
    so its facts are installed as they stand under the instance's own
    statement; each parameter's deletability is judged again at its type,
    as the declaration's check does. Each capture, the callable's
    environment, and each call's capture accesses name the instance's own
    bindings; a capture's type is substituted, since it may be a parameter
    type (`{imm item}` over `item: Self.T`).

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
local derives (`assets/ok/template_overload_binding_local.mojo`), and so does
one that binds a local of a parameter type and hands it to the call
(`assets/ok/template_def_value_local.mojo`, class `FunctionBody`). A struct
method calling a module-scope overload set is the residue filed in roadmap
section 3.

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
  A call to an explicit destructor (2026-09-25) took `stdlib_heavy` from
  1624 installed derivations to 1664, and its reused templates from 655 to
  685: `Dict.pop` (both overloads), `Dict.clear_with`, and `Dict.deinit_with`
  derive for both `Dict` instances, and `Allocation.unsafe_leak`,
  `ThinAllocation.unsafe_leak`, `Allocation.into_thin`, `Set._take_items`,
  `Set.difference_update`, `Set.symmetric_difference_update`, and
  `DictEntry.reap_value` are reused as templates. `Dict.clear_with` and
  `Set.clear_with` are not reused as their own templates, since the
  call-through residue obligation judges the identity substitution's
  parameters as loan carriers; `dealloc` is a module-level `def` with a `var`
  parameter, outside its class. A user struct whose methods consume
  a local, a field of `self`, or a bounded parameter through a named
  destructor or a `var self` method derives for two instances
  (`template_method_explicit_destroy_call.mojo`).
  A consuming call on a copied receiver (2026-09-25), with a direct call of a
  scalar module function beside it, moves `stdlib_heavy` from 1664 to 1720
  installed derivations and from 1938 to 1882 inferred clone bodies: the
  `ContiguousSlice` overload of `List.__getitem__` derives for every
  instance. That overload of `Span.__getitem__` still copies its receiver
  into a local (`var result = self`), and the `StridedSlice` overload of
  `List.__getitem__` reads a tuple element of a local, both outside the
  method grammar. A user struct whose methods call `or_else` on a field of a
  parameter, of `self`, on a local, and on a parameter built over its
  parameter, and a `var self` requirement through a bound, derives for two
  instances (`template_method_copied_consuming_receiver.mojo`).
  A folded value surviving into an instance (2026-09-25) moves no bundled
  count (`stdlib_heavy` installs 1624 derivations before and after): no
  bundled `def` reads a loop variable or value parameter as a runtime value.
  A keyed `def` that accumulates, assigns, prints, or passes its loop
  variable and reads its value parameter derives for every trip count
  (`template_folded_value.mojo`).
  Closed `SIMD` constructions (2026-09-25) move `stdlib_heavy` from 1730 to
  1742 installed derivations, from 1872 to 1860 inferred clone bodies, and
  from 695 to 700 reused templates, and `generic.mojo` from 382 to 388
  installed derivations: `Optional.__hash__` derives for every instance.
  `Dict.__hash__` still constructs its `H2()` (`ConstructTypeParam`) and
  computes on `UInt64` locals, which are not grammar scalars, and
  `path.expandvars` returns a `String` and unpacks a tuple. A user struct
  whose methods hand a `UInt8` tag and a `UInt64` local to
  `_update_with_simd` and pass a `UInt8` construction to a sibling derives
  for an `Int`, a `String`, and a user-struct instance
  (`template_method_simd_construction.mojo`).
  Truthiness conditions (2026-09-25), with `Bool(x)` of a closed struct
  place and a reference call on a `var` local beside them, move
  `stdlib_heavy` from 1748 to 1760 installed derivations, from 1866 to 1854
  inferred clone bodies, and from 705 to 715 reused templates:
  `OptionalReg.or_else` is reused and `List.index` derives for every
  instance. `os.mkdir`, `os.remove`, and `os.rmdir` test a struct too but
  stay outside for a `raises` module `def` or a non-read parameter. A user
  struct whose methods test a field, a parameter, a `var` parameter in a
  `while`, and a local derives for two instances
  (`template_method_truthiness_condition.mojo`).
  Tuple unpackings (2026-09-25) move no bundled count (`stdlib_heavy`
  installs 1754 derivations before and after): `os.removedirs` no longer
  records a table without a recipe, but is a module-level `def` that raises,
  and `path.expandvars` returns a `String` and records a view result's
  interiors. A user struct whose methods unpack a parameter, a sibling
  call's result, and a tuple into declared locals, with a discarded `_`
  element, derives for an `Int`, a `String`, and a user-struct instance
  (`template_method_tuple_unpack.mojo`). A derived instance used to keep a
  closed public `Tuple[Int, Int]` where its clone check names
  `Tuple$t2[…]`, which verification caught on any `Tuple[Self.T, …]`
  parameter.
  Closed `SIMD` grammar scalars (2026-09-25) move `stdlib_heavy` from 1754
  to 1764 installed derivations and from 1848 to 1838 inferred clone
  bodies: `Dict.__hash__` derives for both `Dict` instances. Its clone key
  names `AHasher` because that is the `Dict`'s own `H` struct parameter, so
  its method binder `H2` stays symbolic and its `H2()` derives; what kept it
  was the `UInt64` arithmetic. `Set.__hash__` now passes the grammar and
  keeps the clone check for its `hash(e)` call alone. The grammar reads a
  `SIMD` value whose dtype and width are closed as it reads `Int`
  (`grammar_scalar`), as an operand, a local, a store, and a bool-lane
  condition (`n != 0` over a `UInt32`), whose truthiness mark no instance
  changes. A struct's value parameters stay the four closed scalars. A user struct whose methods mix a `UInt64` local,
  store it back to a field, count a `UInt32` down in a `while`, test an
  `Int32` in an `if`, double a closed vector field, and pass a `UInt64` to
  a sibling derives for an `Int` and a `String` instance
  (`template_method_simd_scalar.mojo`).
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
  Admitting a call result, a nested operator, and a right-hand literal as
  operands (2026-09-25) needed no fact at the temporary: `infer_infix`
  records nothing at an operand, and `check_consuming_as` copies only a
  place, in the template and in a clone alike. What changed is the recipe:
  it reads the two operand types apart, dispatches the struct arm from both,
  and keeps a copy or a conversion the template's own dispatch recorded
  (a consuming dunder over `Meter[T]` copies a place operand in the
  template already; a literal reaches `Meter[T]` through its `@implicit`
  constructor there). A literal beside a bare parameter is rejected by the
  template check itself, as at the pin, so `MaterializeLiteral` never
  reaches the recipe (`template_method_operator_operands.mojo`).
  Tuple element reads (2026-09-26) move `stdlib_heavy` from 1764 to 1792
  installed derivations, from 1838 to 1810 inferred clone bodies, and from
  720 to 725 reused templates: the `StridedSlice` overload of
  `List.__getitem__` derives for every instance. Which facts a check records
  at `bounds[0]` depends on discovery order, not on the body: before the
  generated `Tuple$t3[…]` is declared the element is typed from the tuple's
  arguments alone, and after it the check calls that `Tuple`'s accessor, so
  a template retained in an early round disagrees with its own reuse in a
  later one unless the recipe rebuilds the call from the instance's type.
  A generated `Tuple` that is predeclared but not yet declared refuses the
  derivation. A user struct whose methods read elements of a closed tuple,
  of a `Tuple[Self.T, Int]`, and of `slice.indices(n)` derives for two
  instances (`template_method_tuple_element.mojo`).
  Another struct's overloaded method on a place built over the parameter
  (2026-09-26) moves no bundled count (`stdlib_heavy` installs 573
  derivations before and after): the bundled callers of `List.extend` are
  `List`'s own methods, whose substitution binds the callee's binders
  already. From a user struct's method the substitution binds that
  struct's binders, while the callee's declared signature names its own, so
  `method_clone_target` substituted nothing and compared `List[T]` with the
  clone family's `List[Int]`. It now reads the signature at the receiver's
  arguments first. A user struct over `List[Self.T]` extending a field and
  a `var` local derives for two instances
  (`template_method_foreign_overload.mojo`).
- Hello World mints no per-instantiation clones at all. Its generated bodies
  are members of structs specialized whole and per-call clones, from
  concrete-only templates.
- The census (`template_census.*`) says which table recipes come next: of
  the 244 clone bodies still inferred, 179 are capturable already, and the
  largest blockers left are a binding the grammar refuses (16, a local of a
  parameter type), `ExplicitDestroyCalls` (13, sole blocker of 8; it has a
  recipe since),
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

- A method body beyond `MethodBody`: a receiver origin naming anything but
  one of the method's own origin binders, a copy or move
  initializer, a raised string or a raising call, the method's own binders,
  a `var self` method returning `self^`, a consuming call with a defaulted
  argument, a receiver copied into a local (`var result = self`), a tuple
  element read of a local, a `for` loop with an `else` clause or over an
  iterable whose type an instance changes in kind, a string
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
  `self`. A struct with a value parameter of any type but a closed scalar
  (`DType`, `String`) keeps its methods outside the class.
- An augmented element store through a value getter and a setter
  (`self.table[i] += 1` where `__getitem__` returns a value): the setter binds
  the computed value at a synthesized source span, `(source, DUMMY_SPAN)`,
  shared by every such store in the module, which is no occurrence of the
  body, and records temporaries there. A struct element's `+=` through its
  `__iadd__`, and a store on `self` itself (`Dict.update`'s
  `self[k.copy()] = v.copy()`) likewise.
- An instance whose argument may carry a loan, which includes every struct
  with a field of a parameter type (`DictEntry[K, V, H]`).
- Members of a struct specialized whole, which leave no trace; a per-call
  clone folding a method's own value binder (`scaled[n: Int]`), which the
  method grammar refuses; and a SIMD-keyed hasher leaf
  (`_update_with_simd(value: SIMD[_, _])`), whose template is a trap stub.
- A surviving trait-bound `def` template whose body constructs a struct,
  calls a bound builtin, or consumes a local through a method
  (`hash_seeded`), or holds any feature outside `FUNCTION_FEATURES`. A local
  of a parameter type, a `for` loop, and a whole-value result are covered
  (`assets/ok/template_def_value_local.mojo`).
- Any call that records a conversion, an adjustment, or an origin, and a
  transfer residue: a call-through residue that names a compile-time
  callable (`Tuple.deinit_with[elt_handler]`) or whose argument carries an
  origin, a named callable's own effects behind a residue, a function
  value's baked effects, a destination a captured binding names, and an
  effect whose source is a union. The
  `CallableCaptureAccesses` adjustment is a concrete caller's fact: a
  `capturing[_]` parameter's environment stays open in every clone, so no
  template records it.
- An operator whose left operand is a literal (`1 + self.bag`), which
  dispatches the reflected dunder and records it at the operator in the
  template too.
- A bound dispatch whose instance witness overloads the requirement with
  members of one arity the conversion count does not rank (a rival with
  binders or defaults, a tie, or an argument whose type may come from the
  parameter), and a `var self` requirement called on a place rather than a `^` transfer
  (an implicit copy). A struct's reflective `__hash__` default derives: the
  trait-default expansion declares it before any body is checked. A
  `hasher.update(x)` on a concrete hasher is a Mojito-only spelling (the
  pin's `update` takes bytes), so no fixture exercises it.
- A generic module function called from a method (`hash(e)` in
  `Set.__hash__`), whose instantiation the grammar refuses.
- A folded name beside a literal (`i * 10`) or under a prefix (`-i`), which
  the instance's check folds to one literal; a value parameter of a body
  with no compile-time control flow, which source validation never checks;
  and a folded name lent to an owned parameter.
- A local declared inside a `comptime for`.
- A pack forwarded whole (`print(*a)`), a pack-keyed struct's methods (a
  pack binder fails the method certificate's plain-struct rule, and a
  whole-struct clone leaves no trace), a `rebind`-keyed method,
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
