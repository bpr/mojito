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

Every section is sorted by dependency first, then by how much the entry moves
that section's goal. Entries are numbered `<section>.<n>`; the first unchecked
box is the default next task. The **Model:** bullet is an estimate, not a
sort key. Sections interleave: section 1 does not wait for the others, and
section 3 reopens at every re-pin. Section 5's last entry is a release gate
and stays last.

## Ordered Work

### 1. Front-End Groundwork For A Pliron-Centered Architecture

The assessment is [`docs/pliron-future.md`](pliron-future.md): Pliron cannot
give Mojito Mojo's shape on its own, because Mojo type-checks parametric code
before instantiating it while Mojito elaborates first and checks the clones.
These tasks fix that order.

Scope: only work that moves the check order — checking a template with its
parameters symbolic, or deriving an instantiation from a checked template. A
defect found on the way is filed by its kind; a divergence from the pin goes
to section 3, however small.

- [ ] **1.1 A bound `__hash__` on a vector instance derives where the clone
  check rejects it**

  Problem: `self.value.__hash__(hasher)` in a generic struct's method derives
  for a `SIMD` instance (`T = SIMD[DType.float32, 2]`, `Float32`, `UInt8`),
  and that instance's own clone check rejects the same call.
  - `mojito run` accepts the program, while `mojito check` and
    `MOJITO_VERIFY_TEMPLATE_FACTS=1` report "type 'SIMD[DType.float32, 2]'
    has no method '__hash__'".
  - Derivation re-selects the witness as a hashed leaf
    (`BoundWitness::HashLeaf` in `realize_bound_dispatch`), which is right
    at the pin, where `SIMD` declares `__hash__`.
  - The clone check's compiler-known `SIMD` method arm (`mc_infer.rs`)
    returns before the builtin `__hash__` arm, so a direct
    `v.__hash__(h)` on a vector is rejected too.
  - Either the clone check gains a vector `__hash__` (3.20's blocker) or
    derivation refuses such an instance; until then the two paths disagree.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.2 A leading-dot contextual member keeps the clone check**

  Problem: `ContextualBases` has no derivation recipe, so a body spelling
  `.Member` against a contextual type is inferred per instance.
  - It blocks no body in `stdlib_heavy` today.
  - The fact is the resolved base type's name, which HIR substitutes. A
    contextual type naming a parameter resolves to a different base per
    instance, so the name must be re-rendered from the substituted type.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.3 An operator with a literal or a call-result operand keeps the
  clone check**

  Problem: `BodyShape::operator` admits an operator only over two places, so
  `self.first == value.head()`, `self.count + 1`, and a nested operator leave
  the whole body outside the class.
  - A place records nothing in either check, which is why the recipe reads
    both operands' retained types and nothing else.
  - A literal operand records `MaterializeLiteral` or the conversion into the
    dunder's parameter type, both of which the recipe could write at the
    operand's own occurrence as `realize_operator` already writes a converted
    place's.
  - A call-result or nested-operator operand is an owned temporary. The
    operator path registers none of the temporary facts a call's argument
    records (`places.rs` is never reached from `operators.rs`), so what the
    recipe owes there has to be established first.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **1.4 An augmented element store keeps the clone check where the
  subscripted value or the element has a parameter-built type**

  Problem: an augmented store through a value getter and a setter derives
  only where the subscripted value's type is closed, and a struct element's
  in-place dunder only where the element's type is closed.
  - `self.box[i] += 1` with `box: Box[Self.T]`, or `self[i] += 1` in a
    generic struct, reads through a getter an instance would retarget to its
    own clone. Realization retargets only the call at the site, which is the
    setter, and keeps the getter as the template selected it
    (`substituted_element_stores`).
  - `self.items[i] += x` with `items: List[Self.T]` selects `__iadd__`
    through `T`'s bound, and no recipe re-selects a contract the store embeds.
  - A whole element read from the same list (`self.items[i] =
    self.items[j]`) now checks the same way for every element type
    (`assets/ok/element_store_from_same_list.mojo`), so only its admission
    into the method grammar is left — `element_store` in `template_facts.rs`
    and the `SUBSCRIPT_STORES` note.
  - `Dict.update` now meets a store on `self`, but its index and value are
    calls on an element reached through a read parameter's field
    (`other.entries[i].key.copy()`), which `expression` does not admit.
  - `Dict._append_new` is kept out by a field read through a `var`
    parameter, an `Int(…)` conversion, and a method call on a local.
  - No bundled body holds `x[i] += …`, so none of this moves
    `stdlib_heavy`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **1.5 A method copying its receiver into a local keeps the clone
  check**

  Problem: the method grammar admits neither `var result = self` in a read
  method nor a field store on that local, so such a body is inferred per
  instance.
  - It blocks the `ContiguousSlice` overload of `Span.__getitem__` in
    `stdlib_heavy`: `var result = self`, then `result._data = …` and
    `result._size = …`.
  - The copy is a copied place, which the method class already owes per
    instance (`BodyShape::whole_value`). The stores go through a local rather
    than `self`, which no store shape names yet.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.6 A tuple element read of a local keeps the clone check**

  Problem: the method grammar admits no subscript of a tuple-typed local, so
  a body reading `bounds[0]` is inferred per instance.
  - It blocks the `StridedSlice` overload of `List.__getitem__` in
    `stdlib_heavy`: `var bounds = slice.indices(self.size)`, then
    `var start = bounds[0]` and its siblings.
  - The element read names a tuple accessor chosen by the element's index,
    which substitution does not change; the element's type substitutes.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.7 A surviving trait-bound template with a local of a parameter type
  keeps the clone check**

  Problem: a `def` template may hold scalar locals and runtime `if` and
  `while`, but a whole value of a parameter type may move only inside a
  method body.
  - `def outer[T: ImplicitlyCopyable & Deinitable](x: T) -> Int` with
    `var kept = x` is outside every class, so its instances are inferred.
  - That is also what keeps the overload re-ranking divergence alive (section
    3, "A clone that is still checked re-ranks an overloaded call").
  - The method class already has the three obligations this needs: the copy at
    a copied place, `Movable` at a transfer, and the deletability of a local
    of a bare parameter type (`template_facts.rs:whole_value`).
  - A `def` has no plain-data obligation yet, and its parameter may be
    instantiated with a type that carries a loan or is a callable.
  - A runtime `for` has a recipe now (`TemplateIteration`), but the `def`
    grammar does not admit one: only a method body's may hold a loop.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **1.8 Every discovery round still infers every uncovered body again**

  Problem: a compilation checks the whole elaborated program once per
  discovery round and once per transfer-effect round, and a checked template
  saves only the bodies it covers.
  - Hello World runs three rounds of two transfer passes, about one second of
    body inference each.
  - The plan's persistent elaboration session and expansion worklist did not
    land, on measurement: the elaborator's per-round invariant state costs
    about 5 ms and elaboration is about 2.5% of a compilation
    (`discovery.*.elaborate.indexes` in `--timings`).
  - The lever is reuse of an unchanged body's facts across passes, which the
    catalog already does for a certified template
    (`template_bodies.reused`). Extending it to every body needs a recipe for
    every fact table, under the identity substitution.
  - The arena is already built once per compilation rather than once per
    round (`DiscoveryResult`).
  - Depends on 1.1 through 1.6.
  - Model: Fable, Planned.

- [ ] **1.9 Binder identity is still by spelling below the checker**

  Problem: a type binder is now its declaration's (`Ty::Param { binder }`,
  `ParamDecl::{Type, Value}.id`), but three readers still bind a type by
  its spelling and would confuse two declarations that both spell `T`.
  - Native monomorphization's `Bindings.types` and `Specializer.enclosing_types`
    (`crates/mojito-native/src/native/mono`) are name maps: the waist's
    instructions (`ConstructTypeParam { param }`) name a function's own binder
    by spelling, and `substitute_ty` reads a `Ty::Param` by `binder.name`. A
    contract binder spelled like the function's own would substitute wrongly.
  - The Tuple closedness check (`src/compiler.rs`,
    `tuple_specialization_type_is_closed_in`) and every `where`-clause
    operand (`GenericConstraint::Conforms { param }`, `ConstraintOperand::Param`)
    name binders by spelling, so a nested contract's `T` counts as bound by an
    enclosing `T`.
  - Every anonymous callable contract's binders share the owner `$callable`,
    which is the alpha-equivalence contracts need at depth 0; a contract
    binder bounded by another contract collides one level down.
  - The elaborator's declaration metadata (`$elaborated`) and a 1.0/1.1 MIR
    artifact (`$mir-1.1:<name>`) still have no owner to give.
  - `demangle_specialization` still rebuilds no length-prefixed value, so a
    specialization keyed by a string or a numeric literal (`s`, `I`, `F`) is
    invisible to it. `unqualified_instance_name` spells such a name
    unexpanded, and the conformance oracle in `traits.rs` answers `false`
    for it rather than consulting the template.
    - Binder identity no longer waits on this. `specialization_template`
      delimits a suffix without rebuilding its values, so `binder_owner`
      canonicalizes a type-keyed clone too (2026-09-24), and
      `demangle_specialization`'s own answers are unchanged.
    - A type or reflected key (`y`, `r`) renders a `Ty` that only a later
      phase parses, so those two stay text-only whatever is decided here.
  - A `ParamId`'s owner is the declaration's *name*, so two overloads of
    one name give their own binders the same id whenever the slots agree.
    Nothing distinguishes them, which is why a pack forwarded whole to a
    same-named sibling (`def tally[*Ts](x: Int, *a: *Ts): return tally(*a)`)
    cannot be told from the self-binding a collected element leaves, and
    rejects with "no overload matches the supplied arguments". A call to
    such a sibling has never worked — before the self-binding was read by
    identity it trapped on the template stub instead — so this is a gap to
    close with the rest, not a regression. The owner needs a per-declaration
    discriminator for an overloaded name, and it is serialized in MIR text,
    so the spelling is part of the decision.
  - Depends on nothing.
  - Each corner is a keyed-map change behind an existing helper.
  - Model: Opus, Not Planned.

- [ ] **1.10 A pack forwarded into a method or into `print` fails at the
  clone**

  Problem: the pin runs `self.take(*a)` and `print(*a)` inside a pack-keyed
  body; Mojito validates the body from its template and then fails the
  per-instantiation clone.
  - The elaborator rewrites a whole-pack forwarding for a top-level `def`
    callee only (`comptime/mono.rs:982`,
    `packs.rs:top_level_whole_pack_forwarding_call`, the `$whole_pack` ABI).
    A method call reaches it as `'a' is not a compile-time type`; `print`'s
    spread survives to the clone check as `call spread outside a specialized
    type pack`.
  - `conformance/probes/pack_forwarding_method_untaken_arm.mojo` and
    `pack_forwarding_print_untaken_arm.mojo` pin both; the pin prints
    `1` `two`.
  - Depends on nothing.
  - The rewrite exists; the method and builtin callee shapes need routing to it.
  - Model: Opus, Not Planned.

- [ ] **1.11 A nested variadic construction is refused on the template
  constructor path**

  Problem: `Outer[T, Int](Variant[T, Int](x^))` inside a generic body reports
  "type mismatch for argument 1 to 'Outer.__init__': expected
  Variant[T, Int], found Variant[(T, Int)]", and the same line inside a
  validated body is refused the same way. The pin runs it.
  - A closed user pack has two spellings. The constructor parameter
    `Variant[*Self.Ts]` is expanded element by element
    (`expand_pack_spread`, `crates/mojito-types/src/types.rs`), while the
    inner construction and every annotation bind the pack as one list
    (`TyArg::Val(CtValue::Tuple)`).
  - `coerces` compares the two by argument count and refuses.
  - The lever is a checker-level canonicalization of a closed user pack to
    the bound-list spelling where `expand_bound_pack` is consumed
    (`checker/generics.rs`). The public `Tuple` is the exception: its
    identity is the element-by-element spelling.
  - A concrete `Outer[Int, Bool](Variant[Int, Bool](True))` in `main` is
    unaffected: the elaborator rewrites both applications before the check.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.12 A per-call method clone and a whole-struct specialization leave no
  trace**

  Problem: the elaborator traces a `def` clone and a per-instantiation method
  clone to their templates, and nothing else it generates.
  - A per-call clone (`kind$y3:Int$y4:Bool`) also bakes the method's own
    parameters. `specialize.rs:per_call_method_clones` has four callers and
    records no `MethodInstanceTrace`.
  - A member of a struct specialized whole (`Tuple$…`, a `DType`-keyed range)
    comes from a template whose validated body is never certified: a `DType`
    binder fails `template_certificate`'s plain-binder rule
    (`checker/template_facts.rs`), and a pack binder fails
    `method_certificate`'s plain-struct rule, so all 19 keyed templates
    source validation checks in `stdlib_heavy` (`TString.write_to`,
    `Tuple.write_to`, …) stay refused there. A keyed method of an ordinary
    generic struct derives now (`MethodFeatures::COMPTIME_CONTROL`); the
    pack recipe a `def` uses — a pack bound to its element list in the
    trace, a folded loop index read back per copy
    (`TemplateClass::PackElements`) — is what that class would reuse once
    such a clone leaves a trace, together with an element read
    `self.storage[i]` and a `__getitem_param__[i]()` call on the pack's
    `Tuple[*Self.Ts]` storage.
  - Hello World's 1320 generated body inferences are all of these two kinds.
  - An untraced body keeps the `$`-name test for its `rebind` selection
    (`template_facts.rs:keeps_symbolic_selection`); a traced one keeps the
    selection only when its template was validated.
  - Depends on nothing.
  - The plan is for the per-call trace.
  - Model: Opus, Planned.

- [ ] **1.13 A loan-carrying instance is never cloned whole, so its replayed
  transfers are unverified**

  Problem: the transfer recipe keeps a source only while the substituted
  type of its binding may carry a loan, but no instance with such an
  argument ever mints a clone, so that arm is exact by construction and
  never exercised.
  - `generate_instance_clones` and the `def` specialization path both keep
    an argument mentioning an origin-slotted struct (`Span[Int, o]`,
    `Pointer[Int, o]`) on the erased path, where the template's own facts
    already serve the body.
  - Once such a clone exists, `MOJITO_VERIFY_TEMPLATE_FACTS=1` on a
    `Bag[Span[Int, origin_of(xs)]]` probe decides whether the source the
    clone check records for a `var value: Span[...]` parameter is the
    parameter's own place, as the recipe assumes from
    `declarations.rs`'s registration.
  - A frame effect whose source is a union of places (`abstract_body_origin`
    over a value built from two loans) has no single binding to judge and
    still refuses capture (`body_transfers`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.14 A `Bool` argument does not infer a `Scalar[dt]` lane**

  Problem: `kind(True)` against a lone `def kind[dt: DType](a: Scalar[dt])`
  reports "cannot infer type parameter 'dt'"; the pin binds `DType.bool`.
  - `solve_value_args` (`checker/generics.rs`) reads the lane off
    `simd_shape`, which has no `Bool` arm: a `Bool` is not a SIMD in Mojito.
  - The pin converts the `Bool` into `Scalar[DType.bool]`, at the cost of one
    implicit conversion, so beside a `def kind[T: Copyable](a: T)` the bare
    parameter still wins (`assets/ok/dtype_keyed_overload_family.mojo`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.15 A method overload binding a lane through `Scalar[dt]` ties with
  a bare parameter**

  Problem: `s.kind(x)` over a `Float64`, where `def kind[dt: DType](self, a:
  Scalar[dt])` shares its name with `def kind[T: Copyable](self, a: T)`, is
  ambiguous; the pin selects the `Scalar[dt]` pattern.
  - Free calls select it: `select_callable_overload`
    (`checker/overload_support.rs`) breaks a full tie by the fewest arguments
    with a lane bound to a bare type parameter (`simd_erasures`).
  - Method selection (`method_calls/selection.rs:score_method_call`) has no
    such term.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.16 A keyed method's template body cannot construct a SIMD at its own
  lane**

  Problem: `def make[dt: DType](self, x: Int) -> Scalar[dt]` whose body
  returns `Scalar[dt](x)` reports "invalid checked program: fn 'Lanes.make':
  register r0 has no checked type"; the same body as a free `def` runs
  (`assets/ok/dtype_value_param.mojo`).
  - Every call retargets to a clone that lowers, but the method's own
    template body still reaches MIR with its lane symbolic, and a SIMD or
    scalar *construction* there types no register.
  - A free `def` never shows this: its template is dropped from the round-one
    program, or stands as a `template_stub`. A method cannot leave its
    struct, so the body has to become an `unspecialized_method_stub` the way
    a `SIMD[_, _]`-keyed one already does (`elab.rs`,
    `synth::is_simd_keyed_method`).
  - Reading the lane (`a + a`, `v.reduce_add()`) lowers fine, so
    `assets/ok/dtype_keyed_method.mojo` covers everything but this.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.17 An inferred `SIMD[dt, _]` width leaks into the binding's declared
  type**

  Problem: `var v: SIMD[DType.int32, _] = SIMD[DType.int32, 4](1, 2, 3, 4)`
  then `len(v)` reports "no matching function in call to 'len'": the
  construction solves its own width from the argument count, but the
  annotation's hole stays unsolved in the type the binding declares.
  - `v[2]` and `v + v` run: only the paths that read a lane count off the
    declared type — `len`, and anything keyed on the width — see the hole.
  - Solve the binding's annotation from the initializer's width rather than
    widening what the wildcard admits: `simd-inferred-width` is on the
    divergence ledger and is to be withdrawn, not grown.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.18 A keyword fieldwise construction of a variadic struct is
  rejected**

  Problem: `Pair[Int, Bool](storage=(1, True))` on an `@fieldwise_init`
  variadic struct reports "'Pair$…' expects 1 argument(s), got 0", and so
  does the bare `Pair(storage=(1, True))`; the pin runs both.
  - The specialization's fieldwise path (`infer_construction`, the
    `fieldwise_init` tail) binds positional arguments only; a plain
    fieldwise struct takes keywords through the synthesized `__init__`.
  - Found while probing the pack inference; independent of it.
  - Depends on nothing.
  - One constructor path, with the pin's verdict in hand.
  - Model: Opus, Not Planned.

- [ ] **1.19 An overloaded witness whose members share an arity keeps the
  clone check**

  Problem: a call through a bound derives for an instance whose type
  overloads the requirement only when one member fits the recorded arguments
  and no other member could take as many
  (`template_facts/bound_dispatch.rs:bound_witness`).
  - Two `total(self, by: Int)` and `total(self, by: String)` members need the
    clone check's ranking on the argument's type, which
    `select_method_overload` makes from expressions, not recorded types.
  - Pinned by `template_method_same_arity_witness_overloads_keep_the_clone_check`
    in `tests/compiler_test.rs`.
  - No bundled type overloads a requirement, so `stdlib_heavy` is unmoved.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **1.20 A binder baked into a generic struct's witness keeps the clone
  check**

  Problem: a concrete hasher handed to a `[H: Hasher]` witness derives only
  when the witness's own struct is not generic.
  - A generic owner keys its per-call clone by the instance and the call
    together (`instance_call_method_clone`), which the recipe would mirror.
  - `List._get_copy` over `DictEntry[K, V, H]` reaches it in
    `benchmarks/compile/stdlib_heavy.mojo`: the `Copyable` witness
    `DictEntry.copy` has no clone of its own while the instance has others
    ("the instance has clones, but not of the witness").
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.21 An overloaded method of another struct called on a place built
  over a parameter keeps the clone check**

  Problem: `self.items.extend(self.items.copy())`, or the same call on a
  `var` local, refuses at realization with "a called method's clone family
  has no member for the selected overload".
  - `List.extend` overloads a `var other: Self` member with a
    `Span[Self.T, _]` one. `method_clone_target` compares the selected
    member's lowered name at the instance's types with each clone member's,
    and finds none.
  - The same call inside `List` itself derives (`List.__mul__`,
    `List.__imul__`), so the gap is in naming another struct's clone family.
  - Not root-caused: the `Span` member's inferred origin and the `Self`
    parameter substituted at the caller's arguments are the suspects.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.22 A module function called with explicit parameters keeps the
  clone check**

  Problem: `self.data = unsafe_alloc[Self.T](0)` is refused by the method
  grammar, which admits a module function's call only as a scalar expression
  with no `[...]` arguments (`BodyShape::expression`).
  - The owned `List.__iter__`, `Optional.__iter__`, and `Set.__iter__` are
    this shape. Their local of a built-over type
    (`var result = _ListOwnedIter[Self.T](…)`) derives now; this store is
    what is left.
  - `Set.__iter__` also hands its construction a field of a field
    (`self.items.data`), which `BodyShape::argument` does not admit.
  - The call records the callee's generic instantiation at the substituted
    arguments, which a recipe would re-key per instance; a method's per-call
    clone derives only on a non-generic receiver, whose request no instance
    changes.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.23 An annotated view binding keeps the clone check**

  Problem: `var span: Span[Self.T, _] = self.items` is refused by the method
  grammar, which admits an annotated `var` only where the value's recorded
  type is the declared one or a conversion is kept at the value
  (`BodyShape::annotated_binding`).
  - An annotation left to inference takes the value's own type, and the view
    conversion borrows its source, so the check records neither relation.
  - A recipe would re-infer the `_` slots and record the borrow per
    instance.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.24 A method called on a sibling call's view result keeps the clone
  check**

  Problem: `return self.entries().size()` calls a method on a temporary view,
  a receiver the method grammar does not admit.
  - Receivers are `self`, a field of it, a `var` local, or a reference; a
    sibling view bound to a local first (`var view = self.entries()`) derives.
  - A temporary receiver records its own read-temporary and borrow facts,
    which a recipe would have to key at the call.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.25 A sibling call returning an immutable view keeps the clone check**

  Problem: wrapping a sibling's `View[Self.T, ImmOrigin(origin_of(self))]`
  records an immutable binder (`ConstructionImmutableBinders`), and the
  capture refuses any non-empty record ("a construction binds an origin
  immutably").
  - A record names an origin slot and no binding, so it could be kept
    verbatim, as `CallResultOrigins` is kept by template owner.
  - The capture keeps that table only as the fact that it is empty, which
    installation writes again at each construction, or as a view-returning
    call's immutable result slots, which installation derives again from
    `CallResultOrigins`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.26 A reference result reached through an element's field keeps the
  clone check**

  Problem: `return self.entries[i].value` returns a field of a subscript's
  element, a place `BodyShape::returned_place` does not admit, so
  `Dict.__getitem__` is inferred per instance.
  - The element itself (`return self.items[i]`) derives, through its getter's
    reference call.
  - The path below the getter's reference is the statement's syntax, and the
    referent's type substitutes.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.27 A raise the method grammar does not name keeps the clone check**

  Problem: a raising method derives only where each `raise` names a
  construction or `Error("…")` and no call in its body raises.
  - `raise "…"` becomes an `Error` because its operand is a string, which the
    literal's syntax decides, but no recipe keeps it.
  - A raising callee records `EffectFacts { raises }` at its call, and the
    certificate refuses any effect that is not the default
    (`effects_closed`), though that effect substitutes.
  - A module-level `def` that raises is still outside its class. Every
    bundled one (`os.remove`, `path.getsize`, …) is outside it for other
    reasons too.
  - `os.removedirs` records no fact table without a recipe since tuple
    unpackings derive, so the class is the first refusal it meets. Its
    calls of the generic `rmdir` and `split` are untested until then.
  - No bundled body needs any of these today.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.28 A folded name beside a literal keeps the clone check**

  Problem: over two literals an operator folds, so `i * 10` or `-i` in a
  `comptime for` body types as `IntLiteral` in the instance where the
  template typed `Int`, and `BodyShape` refuses the body.
  - The recipe would give the operator occurrence the literal type and its
    `MaterializeLiteral`, and give its operands a literal type with nothing
    else. That is what the clone check records there.
  - `nested[n, m]` with `sum += i * 10 + j` is the probe.
  - A folded name lent to a `var` parameter is refused too, since what an
    owned literal argument records there is not established.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.29 A `rebind`-keyed method keeps the clone check**

  Problem: `method_certificate` refuses a method source validation checks
  because of a `rebind` over the struct's parameter
  (`rebind[Int](self.value)` under `comptime if Self.T == Int`), so each
  instance is inferred.
  - A keyed `def` with an erased `rebind` derives (`ScalarBranches`), the
    instance owing `TemplateObligation::RebindEqualities`; the method class
    (`COMPTIME_CONTROL`) does not admit one yet because the obligation's
    discharge for a method clone is unexamined.
  - It blocks no body in `stdlib_heavy`: `Tuple.__contains__` is a pack
    struct's, refused earlier (1.12).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.30 A value-keyed `def` with no compile-time control flow keeps the
  clone check**

  Problem: source validation checks a body only when it holds `comptime if`,
  `comptime for`, or a `rebind`, so `def f[n: Int]() -> Int: return n` has no
  checked template, and the executable check refuses a value-keyed body as
  "source validation's to certify".
  - Validating every value-keyed body would give it the folded-literal recipe
    `ScalarBranches` already uses.
  - Validation cost grows by one symbolic check per such declaration.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.31 A module-level `def` with a `var` parameter keeps the clone
  check**

  Problem: the module-level template classes admit only immutable regular
  parameters, so `dealloc(var allocation: Allocation[T])` is inferred per
  instance though its body is one consuming call and one pointer call.
  - Its explicit-destroy call on `allocation^` has a recipe, and
    `MethodBody` already binds a `var` parameter from its declared
    convention.
  - The `def` classes would need the same parameter rule and the
    `CONSUMING_CALLS` receiver.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.32 A body keeping a call-through residue is never reused as its own
  template**

  Problem: `Dict.clear_with` and `Set.clear_with` derive every clone, but
  their templates are inferred again every transfer round.
  - The residue obligation requires each substituted value to be plain data
    (`residue_plain`), and under the identity substitution a parameter is
    still symbolic, so the template role always refuses.
  - The template's own residue names the same slots it republishes, so the
    obligation could hold vacuously for the identity substitution.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.33 A `var self` method returning `self^` keeps the clone check**

  Problem: `return self^` from a `var self` method is outside the method
  grammar, so the owned `List` iterator's `__iter__` is inferred per
  instance.
  - The transfer of `self` is recorded at the transfer and owes `Movable`, as
    a `var` parameter's does; the result has the receiver's own type.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.34 An initializer collecting `var *values` keeps the clone check**

  Problem: a method with a variadic parameter is outside the method
  declaration shape, so the literal initializers of `List`, `Set`, and
  `Array` are inferred per instance.
  - Their loop (`for var value in values^:`) has a recipe now; the refusal is
    the declaration's.
  - The shape admits only regular parameters (`method_certificate`,
    `plain_params`), and its refusal reads "a parameter has a default, an
    origin on a convention other than 'ref', or an 'out' or 'deinit'
    convention", which names none of the variadic case.
  - The collector's type is a `VariadicPack` of the struct's parameter, which
    substitutes; its binding is a parameter the body owns.
  - `Set`'s also has a defaulted `__set_literal__: NoneType = None`.
  - Probe: a generic struct with `def __init__(out self, var *values:
    Self.T)` looping over `values^`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.35 A generic module function called from a method keeps the clone
  check**

  Problem: `hash(e)` in `Set.__hash__` records the callee's generic
  instantiation and overload target, which the method grammar refuses
  (`stray_method_call` rejects any generic instantiation).
  - The body's loop (`for e in self:`) and its `UInt64` accumulator
    (`hash_value ^= …`) have recipes now; this call is what is left.
  - The same call on a parameter-typed field (`h ^= hash(self.value)`) is
    refused earlier, by the grammar itself.
  - The call's selection is bound once in the template
    (`hash$ov$T$Hashable`), and the instance re-keys only the application, as
    1.22 would for an explicit `[...]` application.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.36 A construction whose dtype or width names a parameter has no
  recipe**

  Problem: inference records a `SIMD` construction's dimensions only when
  both are closed, so a template's `Scalar[dt](x)` records nothing, and its
  instance's record must be computed from the instance's arguments.
  - No enabled class reaches such a body today: a `DType` or `Int` value
    binder fails the plain-binder rule, and a struct with a value parameter
    runs its methods on the erased path, so the recipe waits on 1.12.
  - The instance's dimensions are the substituted construction type's
    (`simd_shape`), as a closed construction's are its recorded type's.
  - The method grammar admits only a recorded construction
    (`BodyShape::simd_construction`), so until then such a body keeps the
    clone check.
  - Depends on 1.12.
  - Model: Opus, Not Planned.

- [ ] **1.37 A field read of a field or of a `var` local keeps the clone
  check**

  Problem: the method grammar reads a field only of `self` or through a
  reference (`BodyShape::receiver_field`, `reference_member`), so
  `return self.scaler.base` or `var local = Scaler(1); return local.base`
  keeps the body's clone check.
  - Found beside the parameterized method call recipe: a body calling
    `self.scaler.scaled[3](x)` derives, and the same body reading
    `self.scaler.base` does not.
  - `Set.__iter__`'s `self.items.data` argument is the same gap (1.22).
  - A field of a closed nominal place has the same type and path under every
    instance, so only the place's binding changes.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.38 A string literal passed to a method's `StringSpan` parameter
  keeps the clone check**

  Problem: `var view = self.name.rstrip(" z")` is refused by the method
  grammar, whose argument rule admits a closed scalar, a whole value of the
  parameter's own type, or a converted value, and a string literal is none.
  - The view the call returns derives without the argument
    (`self.name.strip()`), keeping its interior tags and immutable binder.
  - The literal's conversion to `StringSpan` is fixed by its syntax and the
    callee's declared parameter, which a nominal receiver's method keeps
    under every instance.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.39 A nested `def` beyond closed scalars keeps the clone check**

  Problem: a nested `def` derives only over closed scalar parameters and
  result, with explicit `imm`, `mut`, or `var` captures of the method's own
  locals and parameters (`BodyShape::nested_def`).
  - A parameter or result of a struct parameter's type, a capture-all
    default (`{imm}`), a `ref` or transferred capture, a capture of `self`,
    and a nested `def` inside another keep the clone check.
  - The declaration's facts are already keyed by its statement and carried
    by `TemplateNestedDef`; a symbolic signature would substitute there, and
    its parameters' deletability is already judged per instance.
  - A transferred capture moves at the declaration, so it owes `Movable` and
    a transfer fact per instance, as a `^` argument does.
  - It blocks no body in `stdlib_heavy` today.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.40 The Pliron pivot has no falsifiable proof yet**

  Problem: [`docs/pliron-backend-pivot-plan.md`](pliron-backend-pivot-plan.md)
  stages a migration to a required Pliron IR framework, but its Stage A1 slice
  has never been built, so the decision rests on paper.
  - Build the A1 vertical slice from that plan's §Smallest falsifiable proof.
  - Measure construction time, verification time, peak memory, and text size
    against the budget the plan names.
  - Decide from the measurements: continue to A2, or record the rejection in
    [`docs/non-goals.md`](non-goals.md).
  - This is the decision point for MIR-as-a-dialect, not a commitment to it.
  - The attribute layer [`docs/pliron-future.md`](pliron-future.md) found
    missing is landed as independent Rust data
    (`docs/notes/param-expr-attributes.md`); A1 carries it as `#[pliron_attr]`
    wrappers over `uniqued_any`, with its own construction, substitution,
    identity, cross-context clone, and canonical-text tests. That alone is
    not the proof. Last in this section.
  - The plan and the measurement design are the broadest scope in this document:
    they reopen the waist and the dialect policy. The A1 slice is built and
    measured once planned.
  - Model: Fable, Planned.

### 2. Native Backend

The ABI-bump collector is last whatever else moves, because it batches every
change that needs a new `MJRT_ABI_VERSION`.

- [ ] **2.1 Front end: a bare literal cannot build a multi-lane SIMD field**

  Problem: `P(1)` for a struct whose field is `SIMD[DType.int32, 4]` is
  rejected with a field type mismatch.
  - Upstream accepts it through the implicit `SIMD(IntLiteral)`
    initializer.
  - Spell `P(SIMD[DType.int32, 4](1))` until then.
  - The coercion rule itself is one site, but it fires at every field
    initialization, so the plan's job is to bound the overload-resolution and
    fixture fallout before the edit.
  - Model: Opus, Planned.

- [ ] **2.2 Operator operands and the bundled `hash` body take lifecycle copies**

  Problem: `b == c` on a struct whose copy constructor prints prints
  `copy` for each operand, and `hash(b)` prints it twice, on both backends.
  - Upstream's `__eq__(self, other: Self)` and `__hash__` borrow their
    operands; nothing is copied.
  - A named binary-operator operand lowers as a `var.use` copy: the
    checker's `borrow_nominal_place_argument` mark, which `len`, `print`
    and the conversion builtins record, is missing on the operator path.
  - `hash(b)` borrows at the call site; the copies happen inside the
    bundled generic `hash[T: Hashable, ...](value: T)` body in
    `stdlib/std/hashlib/hash.mojo`, so that body (or the `T`-typed
    parameter read it makes) is the second lever.
  - Probe first: a `Box` owning a `List[Int]` with a printing
    `__init__(out self, *, copy: Self)`, compared `==` and hashed,
    verified against the pin.
  - One recording site plus one stdlib body, but the plan bounds the fixture
    fallout of any `__eq__` that relied on the copy.
  - Model: Opus, Planned.

- [ ] **2.3 A value argument forwarded from a caller's parameter is not a native
  constant**

  Problem: a generic `def` that calls another with a value argument built
  from its own parameter runs on the VM and is refused natively:
  `successor[n, 1 + n]()` reports ``in `successor`: unsupported value
  parameter `m` is not compile-time constant``, and a method's
  `below[Self.n, k]()` the same for `n`.
  - MIR passes a value argument as a runtime register (`MirParamArg`), which
    the VM reifies per call. Native monomorphization needs the constant, and
    the register tells it nothing.
  - Forwarding a free `def`'s own parameters unchanged (`below[n, k]()`)
    already works.
  - The lever is a `ParamExpr` on the MIR value argument, which
    monomorphization evaluates under the caller instance's bindings
    (`mono/symbolic.rs::eval_ct`).
  - `assets/ok/param_expr_where_assumption.mojo` avoids the shape so it stays
    in the native parity set; the checker test
    `param_expr_residual_is_not_false` covers it.
  - It changes the MIR call contract and the text schema with it.
  - Model: Fable, Planned.

- [ ] **2.4 A struct instance over a generic instance mangles its constructor
  into an existing symbol**

  Problem: `Bag[List[Int]](List[Int]())` runs on the VM and the native
  backend refuses it: ``in `Bag.__init__`: unsupported concrete instance
  symbol `Bag$mono$TList$u24$mono$u24$TInt$Int.__init__` collides with an
  existing declaration``.
  - The per-instantiation `__init__` clone and the monomorphized constructor
    of the nested application mangle to one name.
  - `Bag[Int]` and `Bag[String]` are fine; only an argument that is itself a
    generic instance collides.
  - `conformance/probes/native_nested_generic_instance_ctor.mojo` pins it;
    the pinned Mojo runs it.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **2.5 Consuming a field of a `deinit self` whose type has droppable
  fields is refused natively**

  Problem: `self.lease^.release()` in a `deinit self` method runs on the VM,
  and the native backend refuses it with "unsupported place consumption with
  droppable fields" when the field's type holds a `String`.
  - Pliron lowers `ConsumePlace` only for a type whose fields need no
    destruction; it would need to destroy the residual fields a named
    destructor leaves.
  - A field with no droppable fields (`Allocation`'s `_alloc`) already
    lowers.
  - `conformance/probes/native_consume_field_with_droppable_fields.mojo`
    pins it; the pinned Mojo runs it.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **2.6 Native runtime ABI bump: land every change that needs a new
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
  - Model: Fable, Planned.

### 3. Catch Up To Current Mojo *(recurring — reopens at every nightly re-pin)*

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

Sorted by how far the entry is from the pin's answer: a program Mojito runs
to a wrong result or accepts where the pin rejects comes first, then one it
rejects where the pin runs it, then a verdict that is right with the wrong
words. The representation gap and the two standing ledgers are last.

- [ ] **3.1 A view returned by a method on a `List` element reads freed memory on
  the VM**

  Problem: `var v = ys[0].rstrip()` followed by `String(v)` fails with "use
  after Pointer deallocation", where the pin prints the stripped text.
  - The element receiver is an `Index` place: the view-result borrow in
    `aggregate_origins` (`checker/origins/ref_params.rs`) lends only
    identifier and field receivers, and MIR materializes only non-place
    receivers, so nothing keeps the element's bytes alive for the view.
  - The call-result aliasing rule already projects such a view through the
    element place (`carried_argument_origins`), which is the origin the
    borrow should carry.
  - Pinned by `conformance/probes/list_element_view_method_result.mojo`.
  - Lending an element place changes loans for every view-returning method on a
    subscript, so the plan enumerates that fallout first.
  - Model: Opus, Planned.

- [ ] **3.2 A reference returned through an origin binder outlives its argument**

  Problem: `print(words.pick(w))` prints `None` at `w`'s last use, where
  `pick[o: Origin](self, ref[o] x: Self.T) -> ref[o] Self.T` returns its
  parameter. It is a silent wrong answer.
  - The pin keeps `w` alive until the returned reference is read and prints
    `w`.
  - Mojito destroys `w` before the read: the result's origin is the binder
    `o`, and the call does not tie it back to the argument it was inferred
    from.
  - An earlier use of the same call prints correctly, because `w` is still
    live there.
  - Probe: `conformance/probes/origin_binder_result_keeps_argument.mojo`.
    `assets/ok/template_method_origin_parameter.mojo` reads `w` afterwards to
    stay clear of it.
  - The plan must say where a call's inferred origin arguments become loans on
    its result.
  - Model: Opus, Planned.

- [ ] **3.3 A returned reference may declare a wider origin than its place**

  Problem: the pin compares a returned place's origin with the declared one
  exactly, and Mojito accepts any declared origin the place lies within.
  - `def peek(ref self) -> ref[origin_of(self)] Self.T` with `return
    self.item` runs on Mojito. The pin reports "cannot return reference with
    incompatible origin: 'origin_of(self.item)' vs 'origin_of(self)'".
  - A `List` subscript is the same: the pin wants
    `origin_of(self.items)._get_owned_interior["element"]`, and Mojito also
    takes `origin_of(self.items)` and `origin_of(self)`.
  - The lever is the return check's `origins/subst.rs:origin_is_within`
    (`statements.rs`, `StmtKind::Return`).
  - Pinned by `conformance/probes/reference_return_wider_origin.mojo`.
  - Every fixture and bundled accessor that declares an owner's origin for a
    field must be found and respelled first.
  - Model: Opus, Planned.

- [ ] **3.4 A store to a field of `self` under a live view of it is accepted**

  Problem: in a `mut self` method, `var view = self.name.strip()` then
  `self.name = String("q")` then `view.byte_length()` runs on Mojito, where
  the pin reports "origin was invalidated here" at the store.
  - The same store through a local (`h.name = …` in `main`) is rejected as
    "access to 'h.name' conflicts with live reference 'view'", so only the
    `self` root escapes the check.
  - The view's origin is the receiver field's owned interior
    (`ViewResultInteriors`), which the store should invalidate.
  - It is the same with and without a struct parameter, so it predates the
    template derivation of such views.
  - Pinned by `conformance/probes/self_field_store_under_live_view.mojo`.
  - Model: Opus, Not Planned.

- [ ] **3.5 A comprehension element may transfer its owned binder**

  Problem: `[x^ for x in items^]` runs on Mojito, where the pin reports
  "expression does not designate a value with an origin" at `x^`.
  - The pin takes the bare binder (`[x for x in items^]`), which both run.
  - Mojito's comprehension check consumes the element as a whole value
    (`check_consuming` in `check_comprehension`), and a `^` on an owned
    binder passes as the move of a local.
  - Pinned by `conformance/probes/comprehension_binder_transfer.mojo`.
  - Model: Opus, Not Planned.

- [ ] **3.6 A literal passed to a `ref` parameter stops at run time**

  Problem: `look(3)` against `def look(ref other: Int)` is accepted and then
  fails with "reference binding to a non-place expression".
  - The pin materializes the literal and binds the parameter to the temporary.
  - Probe: `conformance/probes/literal_to_ref_parameter.mojo`.
  - Model: Opus, Planned.

- [ ] **3.7 `hash` of a struct that overloads `__hash__` stops at run time**

  Problem: `hash(Twin(1))` for a struct declaring `__hash__` for `Some[Hasher]`
  and for a concrete `AHasher` passes the checker and then fails with "vm:
  unknown method 'Twin.__hash__'", where the pin prints the hash.
  - The call reaches the VM under the method's plain name, which no member
    of the overload set is lowered under.
  - Found while probing overloaded witnesses for a bound dispatch.
  - Pinned by `conformance/probes/overloaded_hash_through_hash.mojo`.
  - Model: Opus, Not Planned.

- [ ] **3.8 A method called on a borrowed comprehension binder loses its
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
  - Model: Opus, Not Planned.

- [ ] **3.9 Unpacking a tuple with a `String` element frees the element twice**

  Problem: `var t = (String("t"), 6)` then `var s, n = t` stops with "use
  after Pointer deallocation" on the VM, where the pin prints `t 6`.
  - The same unpack of a `Tuple[String, Int]` parameter runs on the VM and
    stops with "double free of Pointer allocation" under `--backend pliron`.
  - A `Tuple[Int, Int]` unpacks cleanly, and passing the tuple without
    unpacking it runs on both backends.
  - The element reads are the checker's `CheckedTupleUnpackElement` plan
    (`StmtKind::Unpack` in `checker/statements.rs`), so the lever is how a
    place element read through it is copied into the new binding.
  - Pinned by `conformance/probes/tuple_unpack_string_element.mojo`.
  - `assets/ok/template_method_tuple_unpack.mojo` keeps its `String`
    instance to unpacks of a sibling call's result, which both backends run.
  - Model: Opus, Planned.

- [ ] **3.10 A generic struct's `Tuple` field over its parameter cannot be read**

  Problem: a `Holder[T]` field `var pair: Tuple[Self.T, Int]` stops at
  `some_int.pair[1]` with "cannot index Tuple$t2[y3:Inty3:Int]", where the
  pin prints the element.
  - A non-generic struct's `Tuple[Int, Int]` field indexes cleanly, and
    the checker accepts both.
  - The message is the VM's place projection (`backend/vm/places.rs`).
  - Pinned by `conformance/probes/generic_struct_tuple_field.mojo`.
  - Model: Opus, Not Planned.

- [ ] **3.11 A place passed to a `ref` parameter may alias a pack element**

  Problem: `r(x, x)` against `r[*Ts](ref b: Int, *rest: *Ts)` prints `1` in
  Mojito, while the pinned Mojo reports "aliasing values passed mutably to 'b'
  argument and passed immutably to 'rest' argument".
  - The pin infers the `ref` parameter mutable from the mutable place, and a
    pack element is held by reference.
  - With a regular `c: Int` in place of the pack both compilers accept the
    call, because a trivial read parameter takes a copy.
  - Mojito accepts a program the pin rejects, so this is a divergence.
  - Pinned by `conformance/probes/ref_argument_aliases_pack_element.mojo`.
  - The plan must say whether the within-call exclusivity check or the `ref`
    mutability inference is what is missing.
  - Model: Opus, Planned.

- [ ] **3.12 A list literal beside a `List` parameter and a pack is not ambiguous**

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
  - Model: Opus, Not Planned.

- [ ] **3.13 Methods cannot overload on the parameter convention alone**

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
  - Model: Opus, Planned.

- [ ] **3.14 A trivial value handed to a `var` parameter beside a pack is always
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
  - Model: Opus, Planned.

- [ ] **3.15 A trivial rvalue handed to a `var` parameter beside a read overload is
  accepted**

  Problem: `q(x + 1)` against `q(var a: Int)` beside `q(a: Int)` is "ambiguous
  call to 'q'" at the pin, while Mojito selects the `var` overload.
  - The pin accepts the literal `q(7)`, the place `q(x)` and the transfer
    `q(x^)` in the same program, so only the non-literal trivial rvalue is
    ambiguous. No rule was recovered.
  - This is the non-variadic sibling of 3.14, and the two
    disagree: beside a pack the pin calls a bare literal ambiguous, here it
    accepts one.
  - `ArgumentBinding`'s `undecided` bit models the pack rule and is deliberately
    kept off non-variadic candidates, since applying it would reject `q(7)`.
  - Pinned by `conformance/probes/overload_var_trivial_rvalue.mojo`.
  - The plan must find the rule with more probes, or move the entry to
    `docs/non-goals.md` as a kept divergence.
  - Model: Opus, Planned.

- [ ] **3.16 An exact constructor overload loses to its generic sibling as
  ambiguous**

  Problem: `Tag[Int](3)` on a struct declaring `__init__(out self, n: Int)`
  beside `__init__(out self, n: Self.T)` prints `int` at the pin, while
  Mojito reports "invalid call to 'Tag': ambiguous overloaded constructor
  call".
  - Upstream picks the more specific signature. Mojito's construction
    selection scores the substituted `Self.T` parameter as an equal match
    (`select_method_overload`, `checker/declarations.rs`).
  - The preference already exists one level down: a per-call clone of a
    generic constructor wins over the template it was minted from
    (`constructor_is_concrete`).
  - Such a family is also the one shape that collapses on the instance, so
    its constructor clone family stays withdrawn.
  - Found while closing the overloaded-constructor-family item; no fixture
    pins it yet.
  - Model: Opus, Planned.

- [ ] **3.17 A constructor overload set drops every generic candidate before it is
  ranked**

  Problem: `C(s)` for a struct with `__init__(out self, var a: String)` beside
  `__init__[T: Writable](out self, a: T)` prints `2` at the pin, where the
  place costs the first candidate a copy, and `1` in Mojito.
  - `decls_are_concrete` (`checker/declarations.rs`) retains only the concrete
    candidates whenever one matches, at both constructor selection sites.
  - The filter is there so a per-call clone of a generic constructor beats the
    template it was minted from (`Variant`, `Cell`). It cannot tell that clone
    from a separately declared concrete overload.
  - Unlike the rank terms, the filter also overrides conversion cost, so a
    generic candidate needing fewer conversions loses too.
  - Pinned by `conformance/probes/overload_var_copy_constructor.mojo`.
  - The plan must say how a clone is told from a declared overload — the minting
    record, or a marker on the clone.
  - Model: Opus, Planned.

- [ ] **3.18 A clone that is still checked re-ranks an overloaded call**

  Problem: the pinned Mojo binds a call inside a generic body once, while it
  checks the body, and Mojito ranks the overload set again for every
  instantiation that takes the clone check.
  - `def outer[T: ImplicitlyCopyable & Deinitable](x: T) -> Int` holding
    `var kept = x` and `return pick(kept)` prints 1 for `outer(3)` where the
    pin prints 2:
    `conformance/probes/template_overload_rebound_in_clone.mojo`.
  - An instance derived from its checked template already inherits the
    template's choice (`assets/ok/template_overload_binding.mojo`,
    `overload-bound-in-generic-body`), also with a scalar local or a branch
    (`assets/ok/template_overload_binding_local.mojo`).
  - The remaining shapes are the ones outside the derivation classes: a `def`
    with a local of a parameter type, and a struct method that calls a
    module-scope overload set. Section 1 carries those entries, and each one
    that lands narrows this.
  - It closes only as section 1's coverage entries land.
  - Model: Fable, Not Planned.

- [ ] **3.19 A module-level `def` converts to a `def(...) capturing[_]` runtime
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

- [ ] **3.20 A user `Hasher` cannot spell `update` the way the pin requires**

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
  - The blocker for the rename is that a sized scalar has no `__hash__` in
    Mojito: `self.v.__hash__(hasher)` on a `UInt8` reports "type 'UInt8' has
    no method '__hash__'", because a scalar leaf reaches the hasher through
    the checker's `Hasher.update` intrinsic and `record_hash_leaf`, not
    through a real `SIMD.__hash__`.
  - Upstream also replaced the pointer-and-length `hash()` overload with
    `hash_bytes(ImmSpan[Byte])`, which lands in the same pass.
  - The levers are `checker/traits.rs` (the `Hasher` requirement set and its
    shape message), `checker/method_calls/mc_infer.rs` (the intrinsic arms),
    and `stdlib/std/hashlib/` (`hasher.mojo`, `_ahash.mojo`, `_fnv1a.mojo`
    plus every `hasher.update(...)` call site).
  - It changes a compiler-known trait's contract and needs a scalar `__hash__`
    before the rename can land.
  - Model: Fable, Planned.

- [ ] **3.21 `Layout` carries its alignment as a runtime field**

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
  - Model: Opus, Planned.

- [ ] **3.22 `repr` of a sized scalar omits or misstates its type name**

  Problem: `repr(Float32(0.5))` and `repr(Int8(3))` print `Float32(0.5)` and
  `Int8(3)` at the pin, but `0.5` and `3` on the VM, and `Float64(0.5)` and
  `3` natively.
  - The VM's `scalar_repr` (`backend/vm/dispatch.rs`) labels only `Int`,
    `UInt`, and `Float64`, so a `SIMD` width-1 value falls through to its
    display.
  - Pliron's `lower_repr_builtin` (`lower/methods.rs`) labels every float
    scalar `Float64(`, `Float16` and `Float32` included.
  - The label is the scalar alias (`Dtype::scalar_alias`); the value text
    stays the float-format divergence the Dragonbox item owns.
  - Model: Opus, Not Planned.

- [ ] **3.23 A call through a `ref` to a callable value is rejected**

  Problem: `for f in fns: print(f(5))` and `ref g = fns[0]; print(g(1))` run at
  the pin over a function display, while Mojito reports "'f' has type ref
  def(Int) thin -> Int and is not callable".
  - Both thin and capturing elements are affected.
  - The indexed call `fns[0](5)` already works through element-call dispatch.
    A `ref`-typed callee has no such path.
  - Model: Opus, Planned.

- [ ] **3.24 Forwarding a named accessor's reference result is rejected**

  Problem: `return self.items.unsafe_get(index)` under
  `ref[origin_of(self.items)._get_owned_interior["element"]]` reports
  "returned reference escapes storage outside its declared origin", where the
  pin runs it.
  - The subscript `return self.items[index]` under the same origin works.
  - Only the subscript path records the interior generation the return check
    compares (`indexing.rs`, `origins/interior.rs:record_interior_reference`).
    A named call leaves the returned reference's place without it.
  - Pinned by `conformance/probes/reference_return_forwarded_accessor.mojo`.
  - Model: Opus, Planned.

- [ ] **3.25 A generic method moving its `var` parameter into a sibling call's
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
  - Model: Opus, Not Planned.

- [ ] **3.26 A reference-returning `def` is not read through as a `print` argument**

  Problem: `print(pick(w))` for a free `def pick[o: Origin](ref[o] x: String)
  -> ref[o] String` reports that `ref String` does not conform to `Writable`.
  - The pin reads through the reference, and so does Mojito for the same call
    on a method.
  - Probe: `conformance/probes/reference_result_print_argument.mojo`.
  - Model: Opus, Planned.

- [ ] **3.27 A binder is not inferred through a converting argument**

  Problem: `boxed(value)` for `def boxed[U: Copyable & Deinitable](box:
  Wrapper[U])` and a `value: T` reports "cannot infer type parameter 'U' of
  'boxed' from the arguments", where the pin infers `U = T` through the
  `@implicit` constructor and runs it.
  - Inference matches an argument's own type against the parameter's before
    any conversion is considered, so a conversion can only reach a parameter
    whose binders are already fixed (a method's `Wrapper[Self.T]`, or a
    closed type).
  - Pinned by `conformance/probes/inferred_binder_through_conversion.mojo`.
  - Model: Opus, Planned.

- [ ] **3.28 A bound call's result converted at an annotated binding fails
  MIR verification**

  Problem: `var x: Optional[T] = v.copy()` in a generic `def` reports
  "register r1 has no checked type" for the `copy` call, where the pin runs
  it.
  - The same binding at a concrete type runs, and so does the result bound to
    an unannotated local first and then converted.
  - Probe: `conformance/probes/bound_call_result_converted_at_binding.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.29 Constructing a struct's own type parameter fails in every instance
  clone**

  Problem: `self.x = Self.T()` in a method of `struct Box[T: Copyable &
  Deinitable & Defaultable]` is rejected for the instance — "in 'reset'
  instantiated for 'Box[Float64]': type mismatch for assignment target:
  expected Float64, found T" — while the pin runs it.
  - A clone respells `Self.T` in its annotations but not in a construction
    expression, so the call still types as the template's `T`.
  - A constructor clone fails the same way, so the shape reaches lifecycle
    and ordinary bodies alike.
  - Found while closing the overloaded-constructor-family item; no fixture
    pins it yet.
  - The plan names the substitution `specialize_method_clone` applies to
    expressions, not only to annotations.
  - Model: Opus, Planned.

- [ ] **3.30 Two type-pack `__init__` overloads cannot be constructed**

  Problem: `H(x, x, x)` for a struct with `__init__[*Ts](out self, a: Int,
  *rest: *Ts)` beside `__init__[*Ts](out self, a: Int, b: Int, *rest: *Ts)`
  prints `3` at the pin and fails in Mojito with "checked constructor
  'H.__init__$ov$…' is missing from MIR".
  - The checker selects the same constructor the pin does. The selected clone
    never reaches MIR.
  - The rejection is safe, but it is a VM-phase message for a program the pin
    runs.
  - Pinned by `conformance/probes/pack_overload_constructor_missing_mir.mojo`.
  - Free functions and methods are served from the checker's recorded selection;
    the plan must find why constructors are not.
  - Model: Opus, Planned.

- [ ] **3.31 A `def`'s own type pack cannot be queried in a runtime position**

  Problem: `return 1 + Us.length` fails with "Undefined variable 'Us'", while
  the pinned Mojo runs it.
  - A free `def count[*Us](var *extra: *Us)` and a method-own pack on any
    struct fail the same way.
  - A compile-time position (`comptime for i in range(Us.length)`) works, and
    so does a variadic struct's own pack (`Self.Ts.length`) in a runtime
    position.
  - Only `generate_struct_spec` binds a pack into the substitutions that
    `fold_pack_typelist_use` (`comptime/rewrite.rs`) reads during
    materialization. `def` specialization never does.
  - Pinned by `conformance/fixtures/pack_length_runtime_position.mojo`
    (`pack-length-runtime-position`, `mojo-only`).
  - It stays in the elaborator, so it does not wait for section 1's symbolic
    pack work.
  - The binding site is known, but the plan must enumerate the clone paths that
    share it (free `def`, method-own pack, nested forwarding) and the
    materialization rewrite each one runs.
  - Model: Opus, Planned.

- [ ] **3.32 A value-returning body that ends in `abort(...)` is rejected**

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
  - Model: Opus, Planned.

- [ ] **3.33 Explicit type arguments on a static method of a non-parametric
  struct are rejected**

  Problem: `P.plain[Int](4)` on `@staticmethod def plain[T: Writable](x: T)`
  prints `1` at the pin, while Mojito reports "Undefined variable 'P'".
  - Any generic static on a struct without parameters is affected, whether
    or not its body holds a `comptime if`.
  - The inferred spelling `P.plain(4)` works.
  - The explicit spelling parses as `Invoke` over `Member(P, plain)`. The
    error is raised before the non-parametric static path in
    `checker/method_calls/mc_infer.rs` sees the call.
  - The plan must first find which pass infers the bare type name as a value.
  - Model: Opus, Planned.

- [ ] **3.34 A compile-time-keyed `def` cannot be passed as a function value**

  Problem: `apply(as_int, 3)`, where `as_int[T]` holds a `comptime if` or a
  `rebind` and `apply` declares a callable bound, runs at the pin and reports
  "Undefined variable 'as_int'" in Mojito.
  - The template is dropped or stubbed, and only its `$`-mangled clones carry
    a name; a bare reference resolves to neither.
  - The rejection is safe (no wrong answer), but the message names nothing
    the source wrote.
  - The plan must say which clone a bare reference names, and what the rejection
    says when none can be chosen.
  - Model: Fable, Planned.

- [ ] **3.35 An associated alias is not constructible through a parameterized
  base**

  Problem: `Holder[7].Same()` for `comptime Same = Sized[Self.n]` reports
  `Undefined variable 'Holder'`, where the pin runs it.
  - The annotation spelling works: `var made: Holder[7].Same = Sized[7]()`.
  - The alias now binds the instance's value parameters
    (`associated_type_from_base`), so only the call path is missing.
  - Model: Opus, Planned.

- [ ] **3.36 A member-led arithmetic type argument does not parse in an alias
  body**

  Problem: `comptime Next = Sized[Self.n + 1]` is a parse error (`Expected ']'
  after a subscript`), where the pin accepts it.
  - The bracket is parsed as a runtime subscript, whose index grammar stops at
    the member access. `Sized[(Self.n + 1)]` and `Sized[0 + Self.n]` parse.
  - The same expression in annotation position (`var x: Sized[Self.n + 1]`)
    parses, and the checker already types it symbolically.
  - It is the standing Index-versus-TypeApply split; the plan decides whether
    the alias body re-parses as a type or the subscript grammar widens.
  - Model: Opus, Planned.

- [ ] **3.37 An arithmetic `where` operand compiles for `def`s and struct methods
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
  - Model: Opus, Planned.

- [ ] **3.38 A tuple binding with a `Tuple[...]` annotation loses `reverse` and
  `concat`**

  Problem: `var t: Tuple[Int, String] = (1, "x")` followed by `t.reverse()`
  or `t.concat(Tuple(True))` runs at the pin, while Mojito reports "type
  'Tuple$t2[y3:Inty6:String][Int, String]' has no method 'reverse'".
  - The same calls work on an unannotated binding (`var t = (1, "x")`) and
    on `var t = Tuple(1, "x")`.
  - The annotation resolves to the minted `Tuple$t2[...]` struct, so method
    lookup reports the missing member before the builtin tuple path
    (`infer_tuple_method`, reached from `method_calls/mc_infer.rs`) or a
    `TupleTransformRequest` for the clone is ever considered.
  - Found in the 2026-09-17 gate triage; no fixture pins it yet.
  - The plan decides whether the annotation should keep the public `Tuple`
    spelling or the lookup should fall through, and lists the other members the
    minted spelling hides.
  - Model: Opus, Planned.

- [ ] **3.39 A reflection method call in a runtime position is rejected**

  Problem: `comptime r = reflect[Point]` followed by `print(r.field_count())`
  prints `2` at the pin, while Mojito reports "Undefined variable 'r'".
  - Binding the result first works: `comptime count = r.field_count()` then
    `print(count)`.
  - The reflection handle erases before the executable check, so a method
    call on it must fold to its compile-time value where it stands, as the
    upstream materialization of an `Int` result does.
  - A direct `reflect[Point].field_count()` in a runtime position fails too,
    with "type 'reflect[…]' has no method 'field_count'".
  - Found in the 2026-09-17 gate triage; no fixture pins it yet.
  - The plan lists which handle results are implicitly materializable (an `Int`,
    a `Bool`) and which still need an explicit crossing (a name list).
  - Model: Opus, Planned.

- [ ] **3.40 `reflect[T]` over a function's type parameter is rejected**

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
  - Model: Opus, Planned.

- [ ] **3.41 `reflect[T]` of a non-struct type is rejected**

  Problem: the pinned Mojo answers `reflect[Int].field_count()` with 0, and
  Mojito rejects it with "requires a struct type".
  - `comptime/eval.rs` raises it for every reflection method over a type that
    is not a struct.
  - `conformance/probes/template_fallback_reflection.mojo` records the
    observation, made 2026-09-20.
  - Which handle methods answer for a scalar, and with what, needs a probe per
    method before the lever is chosen.
  - Model: Opus, Planned.

- [ ] **3.42 Upstream `DType` names with no Mojito dtype are rejected**

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
  - Model: Opus, Planned.

- [ ] **3.43 A parametric nested `def` named as a value reports its marker**

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
  - Model: Opus, Not Planned.

- [ ] **3.44 `String` always owns a heap buffer, where upstream's has three
  representations**

  Problem: `String(literal)` allocates and copies the literal's bytes, while
  upstream points at the static bytes and copies only on the first mutation.
  - Upstream packs a static-constant, an inline (up to 23 bytes), and a
    reference-counted heap form into the same 24 bytes, flagged in
    `_capacity_or_data`; Mojito's `{data, size, cap}` has only the heap form.
  - No program output differs; allocation counts and `capacity()` do.
  - Port the static form first: every mutator, `__del__`, copy, and move
    must respect a non-owning flag, on both backends.
  - Model: Opus, Planned.

- [ ] **3.45 Shift operators bind looser than the bitwise ones**

  Problem: `a << 1 | b >> 1` parses as `(a << 1 | b) >> 1`, where Python and
  Mojo bind `<<`/`>>` tighter than `&`, `^`, and `|`, so it prints `7` for
  `Int32(6)` and `Int32(3)` where the pin prints `13`.
  - Found by `assets/ok/simd_symbolic_surface.mojo`, whose `guarded`
    parenthesizes around it.
  - The lever is the infix precedence table in `crates/mojito-parser`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.46 Small SIMD surface gaps the symbolic-lane probes found on concrete
  types**

  Problem: each of these runs at the pin on a concrete scalar or vector and
  is rejected by Mojito, so a symbolic template cannot license it either.
  - `len(v)` on a vector (`len_result_for_type` has no `Ty::Simd` arm),
    `abs`/`max`/`min` over a scalar alias (`is_numeric` excludes it),
    `Scalar[dt].MAX`/`.MIN`, and `**` on a lane.
  - A scalar comparison types as `SIMD[DType.bool, 1]`, which does not
    coerce to a `Bool` return (`-> Bool: return a == b`); `if a == b:`
    works through truthiness.
  - `Float64.cast[...]()`: the canonical width-one `float64` is `Ty::Float64`
    and has no SIMD methods.
  - Natively only, `String(v)` of a multi-lane vector is an unsupported
    type where `print(v)` lowers (`pliron backend: unsupported type`).
  - On a symbolic lane only, `range(Scalar[dt](0), n)`, `to_bits()` without
    an explicit target, and a `DType.bool` mask's `fill=` at a symbolic
    width are reported at the template rather than deferred to the
    instantiation.
  - Depends on nothing.
  - One site each.
  - Model: Opus, Not Planned.

- [ ] **3.47 Each unrolled `comptime for` iteration shares the enclosing scope**

  Problem: `comptime for i in range(2):` with `var v: Int = i` in its body
  prints `0` then `1` at the pin, while Mojito reports "'v' is already
  declared in this scope".
  - The elaborator splices every unrolled copy of the body into the enclosing
    block (`comptime/elab.rs`, the `ComptimeFor` arm), so the second copy
    redeclares the first's locals; the validator checks the body once in its
    own scope and accepts it.
  - A `comptime n = names[i]` binding in the body fails the same way.
  - Pinned by `conformance/probes/comptime_for_body_scope.mojo`.
  - Depends on nothing.
  - Each iteration needs its own scope, or its locals renamed.
  - Model: Opus, Planned.

- [ ] **3.48 A trait default body holding a `comptime if` never elaborates**

  Problem: a trait whose default method holds `comptime if True:` reports
  "unsupported feature: comptime if" for every conformer, where the pin
  checks the default symbolically and runs it.
  - The elaborator specializes struct methods and `def`s; a default body a
    conformer inherits is never stubbed or selected, so the construct reaches
    the executable check.
  - The upstream reflective defaults (`Hashable.__hash__`, `Equatable.__eq__`)
    are written as such bodies over `reflect[Self]`.
  - Depends on nothing.
  - The plan says where an inherited default is copied into a conformer today
    and where its selection would run.
  - Model: Opus, Planned.

- [ ] **3.49 A field name under a symbolic index prints without `materialize`**

  Problem: `print(names[i])` inside `comptime for i in range(len(names))`
  over `comptime names = reflect[T].field_names()` prints at Mojito, while
  the pin rejects it with "cannot materialize comptime value of type
  'Array[StringSpan[...]]'" and needs `materialize[names[i]]()`.
  - Mojito accepts a program the pin rejects, so this is a divergence.
  - `materialize[names[i]]()` fails at the instance in turn ("Undefined
    variable 'materialize'"): the elaborator's crossing takes a bare binding
    only.
  - Depends on 3.47 for the `comptime n = names[i]` workaround.
  - Model: Opus, Planned.

- [ ] **3.50 `comptime assert` is not parsed**

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

- [ ] **3.51 A field's value cannot be read by reflection**

  Problem: `reflect[T].field_ref[i](x)` — the value of field `i` of `x` — is
  unsupported; the upstream hashing, equality, and writing defaults are
  written with it.
  - Mojito's defaults are Rust AST synthesis (`comptime/synth.rs`) and a
    lowering (`mojito-pliron/src/lower/print.rs`), so no bundled body needs
    it yet.
  - Under validation the result's type is the opaque field type
    (`types[i]`), which the arm-licensing rule already covers.
  - Depends on 3.50 for the upstream spelling of the defaults.
  - Model: Fable, Planned.

- [ ] **3.52 `Int()` has no zero-argument constructor**

  Problem: `print(Int())` prints `0` at the pin, while Mojito reports "'Int'
  expects 1 argument(s), got 0"; `Float64()` likewise.
  - `Int` conforms to `Defaultable` at the pin, so a `conforms_to(FT,
    Defaultable)` arm over an `Int` field selects `FT()` and then fails.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.53 A `var` of an opaque type is not required to be `Deinitable`**

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

- [ ] **3.54 A local compile-time type list is unknown in an instance's annotation**

  Problem: `var v: types[i] = ...` over `comptime types =
  reflect[T].field_types()` validates, but the instance reports "unknown
  type 'types'".
  - The elaborator resolves `f.T` over a bound handle (`resolve_reflected_type`,
    `comptime/eval.rs`) and nothing else in an annotation; `comptime FT =
    types[i]` then `var v: FT` is the working spelling.
  - Depends on 3.47 when the annotation sits in a `comptime for` body.
  - Model: Opus, Not Planned.

- [ ] **3.55 A handle chain in a call's type argument is read as a value**

  Problem: `_unqualified_type_name[reflect[T].field_at[i].T]()` reports
  "expected a type, found a value" at the instance.
  - The elaborator's `resolve_reflected_param_arg` rewrites a `ParamArg::Type`
    only; the chain arrives as a value argument.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.56 A `mut self` witness does not conform to a read-`self`
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
  - Model: Opus, Planned.

- [ ] **3.57 An imported alias in a generic struct's method signature is an
  unknown type**

  Problem: `def feed(self, mut hasher: default_hasher)` on a generic struct
  reports "unknown type '__module$hasher$default_hasher'" for its instance,
  while the pin runs it.
  - The same annotation resolves on a module-level `def`, on a plain
    struct's method, and through a local `comptime` alias of the import.
  - Workaround: spell the application (`AHasher[SIMD[DType.uint64, 4](0)]`),
    as `assets/ok/template_method_bound_witness_shapes.mojo` does.
  - Pinned by `conformance/probes/imported_alias_in_generic_method.mojo`.
  - Model: Opus, Not Planned.

- [ ] **3.58 A subscript store on a struct with a setter and no getter is
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
  - Model: Opus, Planned.

- [ ] **3.59 `@explicit_destroy` is accepted on a type that is `Deinitable`
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

- [ ] **3.60 Mojito-specific shortcuts to move toward Mojo's shape** *(standing,
  any order)*

  Problem: parts of Mojito's stdlib lean on the Rust runtime where upstream
  is pure Mojo. Each is a candidate port, preferred over any new bridge
  (2026-09-07 direction).
  - Float formatting in Rust: the float arm of the VM's `Display for Value`
    and the native `mjrt_fmt_f64`. Upstream formats `Float64` in Mojo
    (Dragonbox in `format_float`). Five corpus fixtures print different text
    on the two compilers because of it
    (`conformance/assets-mojo-output-diffs.tsv`, family `float-format`): the
    pin writes an exponent sign (`1e+23`, Mojito `1e23`) and renders a
    `Float32` at its own precision (`0.1`, Mojito `0.10000000149011612`).
    - Transliterate a permissively licensed Rust Dragonbox (MIT or Apache-2.0 —
      third-party crates are allowed, see `AGENTS.md`) into Mojo rather than
      deriving the algorithm; the plan picks the source and pins the shortest-
      round-trip cases. Only a from-scratch derivation would want Fable.
    - Model: Opus, Planned.
  - `DType` is a compiler builtin (`Ty::Dtype`, with its `is_*` queries and
    display in the VM and the native lowering) where upstream's is a stdlib
    struct over a one-byte code. A bundled `struct DType` needs struct-valued
    associated `comptime` members (`eval_associated_ct` rejects them), runtime
    reads of `StructName.NAME`, struct value parameters on defs, and a bridge
    from a frozen struct value to `Dtype` at every hard-wired `DType` site.
    - Several checker and elaborator capabilities land before the struct can
      replace the builtin.
    - Model: Fable, Planned.

  Four runtime services are deliberately not on that list; they are in
  [`docs/non-goals.md`](non-goals.md).

- [ ] **3.61 Behavioral divergences from the pinned Mojo — burn to zero**
  *(standing)*

  Every new divergence lands here with a probe or a `cases.tsv`
  `mojito-only` / `output-diff` row, and leaves when its probe promotes to
  an `assets/ok` fixture.

  Open today:
  - `len-over-pack-in-comptime-for-header`: `comptime for i in
    range(len(items))` over a runtime pack `*items: *Ts` runs in Mojito and
    is rejected upstream ("cannot use a dynamic value in call argument");
    both accept `items.__len__()` there. Pinned by
    `conformance/probes/pack_len_comptime_for_header.mojo`.
    - The lever is the builtin `len` over a `VariadicPack` under source
      validation and the elaborator's `len(args)` fold
      (`specialize.rs:generate_def_spec`), which answer the arity where the
      pin treats the call as dynamic.
    - Model: Opus, Planned.
  - `result-alias-rule-coverage`: a free function whose return declares an
    owned interior of an argument is not judged by the call-result aliasing
    rule (`checker/origins/result_alias.rs`), so `w = keep(view_x(w))` runs
    in Mojito and is rejected upstream.
    - `view_x(v: W) -> StringSpan[origin_of(v.x)._get_owned_interior["bytes"]]`
      carries its argument's origins unprojected, because only methods record
      `view_result_interiors`; a free call needs the same side table keyed
      by the projected parameter.
    - Free-function signatures keep no source return type the call site can
      read, so the plan picks where the parameter projection is recorded.
    - Model: Opus, Planned.
  - `unpack-assign-call-over-viewed-local`: `a, b = pair(a.rstrip())` runs
    upstream (`ab 1`), while Mojito rejects it with "access to 'a' conflicts
    with live reference '$arg_loan_r5'": the argument's view anchor outlives
    the call into the unpacking store.
    - The anchor's statement-end keep-alive is what every other call argument
      relies on, so shortening it for unpacking needs its fallout checked first.
    - Model: Opus, Planned.
  - `moved-parameter-into-local-collection`: in a generic struct's method,
    `result.append(value^)` into a local `List[Self.T]`, with `value` a `var`
    parameter of type `Self.T`, then `return result^`, runs upstream and is
    rejected by Mojito with "use of uninitialized value 'value'". Pinned by
    `conformance/probes/moved_parameter_into_local_list.mojo`.
    - A parameter whose type may carry loans stands for the caller's loans by
      its own place (`declarations.rs`, the parameter's aggregate origins), so
      `append`'s transfer effect installs a loan on `value` in `result`, and
      the erased generic body reads `value` after it moved.
    - A field of `self` as the destination installs nothing in the frame
      (`install_call_transfers`), and a concrete element type carries no
      loans, so only the erased body with a local destination rejects.
    - The lever is what a moved source's stand-in place means once the
      source is gone: its loans outlive the move, the place does not.
    - Model: Opus, Planned.
  - `trivially-movable-stdlib-types`: `IsTriviallyMovable[String]`,
    `IsTriviallyMovable[List[Int]]`, and the `MaybeUninit` conformances
    that follow from them are `False` on Mojito and `True` upstream,
    because six bundled stdlib types (`String`, `List`, `Array`, `Dict`,
    `Set`, `Optional`) declare an explicit `__init__(out self, *, deinit
    move: Self)` where upstream relies on the implicit bitwise move. The
    predicate itself agrees on hand-written structs.
    - Deleting the six move constructors is the fix, but it hands every heap-
      owning move to the compiler-generated path on both backends, so the plan
      checks that path first.
    - Model: Opus, Planned.
  - `ref-binding-register-value`: a `ref` binding to a register-passable
    value is rejected upstream (`value of type 'Int32' doesn't have a
    memory origin in 'ref' binding`) but accepted by Mojito. It covers
    `ref y = f()` for an `Int`-returning `f`, `ref y = a + 1.0`, and a SIMD
    lane read `ref lane = v[i]`. The lane binding also writes through to
    the vector, so `lane += 2` changes `v`. A memory-backed temporary
    (`ref x = make_list()`) is accepted by both. The lever is the
    `StmtKind::RefDecl` arm in
    `crates/mojito-checker/src/checker/statements.rs`, whose
    `materialized_reference_actual` fallback materializes any value.
    Pinned by `conformance/probes/ref_binding_register_value.mojo`.
    - The lever is named, but the rejection reaches wide fixture fallout, so it
      wants its own pass with the fallout enumerated first.
    - Model: Opus, Planned.
  - `tuple-element-write`: `t[0] = 9` on a `Tuple` runs upstream and prints
    `9`, while Mojito rejects it with "invalid assignment target: Tuple
    elements are immutable". Upstream's `__getitem__[idx](ref self)` returns
    `ref [self]`, so the subscript is a mutable place.
    - The rejection is a checker rule that predates the
      reference-returning `__getitem_param__` the bundled
      `std/builtin/tuple.mojo` now declares, so the declaration and the rule
      disagree about the same hook.
    - Withdrawing it also decides `std/collections/pack_tuple.mojo`, whose
      accessor returns a copied value precisely to keep the write rejected
      (`self_hosted_pack_tuple_preserves_tuple_restrictions`).
    - Pinned by `conformance/probes/tuple_element_write.mojo`.
    - The lever is one rule, but making tuple elements writable changes what
      every tuple place means to ownership analysis, so the plan enumerates that
      fallout first.
    - Model: Opus, Planned.
  - `native-arithmetic-edge-cases`: seven corpus fixtures compute different
    numbers from the pin, found by the 2026-09-12 stdout sweep and listed in
    [`conformance/assets-mojo-output-diffs.tsv`](../conformance/assets-mojo-output-diffs.tsv)
    under `arithmetic`. The clearest is a shift past the bit width
    (`assets/ok/pliron_straightline.mojo`: `a << 65`, where the pin prints 7
    and Mojito 51102306), and the SIMD shift and floor-division fixtures
    disagree wholesale. Also here: an out-of-range float-to-int cast and `Float64` `**`.
    `round()`'s half-way case left this list on 2026-09-13 when `round`
    became ties-to-even on both backends.
    - Each case needs the pin's rule established before Mojito's is changed, and
      `docs/native-abi.md` already defines some of them deliberately (wrapping
      overflow), so the plan decides which are bugs and which are recorded
      choices.
    - Model: Opus, Planned.
  - `method-capturing-callable-parameter`: a method whose compile-time
    callable parameter binds a capturing closure is rejected — "operator Mul
    is not defined for Int and None" — while the pin runs it and the same
    closure through a free function is `assets/ok/lambda_hof.mojo`.
    - The method call is what breaks it: the program runs once the
      `runner.apply[scale](5)` line goes, so binding the closure to a
      *method's* parameter is what retypes the captured `factor` as `None`.
    - Pinned by `conformance/probes/method_capturing_callable_parameter.mojo`.
    - The native side of this shape is unbuilt behind the checker: only the
      direct-call arm promotes a capturing callable argument to a runtime
      parameter (`mono/promote.rs`), so a `MethodCall` carrying one would
      still reject contextually at monomorphization.
    - Model: Opus, Planned.
  - `destructor-timing-against-the-pin`: two corpus fixtures run the same
    destructors later than the pin does
    (`conformance/assets-mojo-output-diffs.tsv`, family `drop-timing`):
    `assets/ok/owned_pointer_api.mojo` and
    `assets/ok/try_region_drop_timing.mojo`. The two `MaybeUninit` fixtures
    that used to skip destructors outright left this list on 2026-09-17, when
    owning temporaries gained their hidden slots.
    - Both remaining cases are orderings rather than omissions, and the plan
      establishes where the pin runs each destructor before Mojito's schedule is
      moved.
    - Model: Opus, Planned.
  - `pointee-element-reference-return`: a method returning a reference into
    what a `Pointer[T, Self.o]` field borrows names the region with the
    struct's own origin parameter. The pin distinguishes the element
    sub-origin (`origin_of(o["element"])`) and wants
    `ref[origin_of(self.src[][i])]`, which Mojito's escape check rejects
    because no `Origin` variant carries a projected `SelfParam`. Two
    `assets/origin_ok` fixtures moved for it, and five `Pointer` iteration
    twins (`assets/extensions/ok/pointer_field_reference_yielding_iteration*`,
    `pointer_field_comprehension_borrowed_named_source`,
    `pointer_field_parametric_mut_iterator_read`) joined their `ref`-field
    originals.
    - The fix adds a projected receiver origin to
      `mojito_types::origin::Origin`, which MIR text, verification, and
      substitution all read.
    - Model: Opus, Planned.
  - `local-comptime-capture`: a function-local `comptime` constant read from
    a nested `def` is an ordinary immutable local in Mojito, so it must be
    named in the capture list; the pin treats it as a compile-time constant
    and rejects naming it there. Mojito cannot simply stop requiring the
    capture: the constant has real storage in the outer frame and the lifted
    function has no binding for it.
    - The fix makes a folded `comptime` local a constant the lifted body can
      read, which is a lowering change, not a scope-rule change.
    - Model: Opus, Planned.
  - `capturing-lambda-argument` / `capturing-lambda-locals` /
    `owned-capture-closure-locals` / `mut-self-callable-struct`: four
    callable-shape leniencies. Mojito passes a capturing lambda as a runtime
    argument, binds one to a local under an explicit `capturing[...]`
    annotation, and accepts a `mut self` `__call__` as a `def(...)`
    conformer. The pin takes a capturing callable only as a compile-time
    parameter (and refuses a lambda even there), types a capturing lambda as
    a plain `def(...) -> T` that converts to nothing, and wants a read
    receiver. Three corpus fixtures moved to `assets/extensions/`; three more
    were respelled onto `thin` contracts and `@parameter def` arguments.
    - The four share one question — what a capturing callable *value* is — and
      the plan settles that before any rejection.
    - Model: Opus, Planned.
  - `interior-generation-view-consume` / `interior-generation-view-drop`:
    a struct field typed
    `Pointer[T, Self.origin._get_owned_interior["tag"]]` over a generic
    origin parameter. The pin parses the projection but calls the interior
    reference never-initialized, so the carrier struct does not type-check
    there at all. Two `assets/ownership_ok` fixtures moved for it.
    - Upstream's owned-interior origins are real; what differs is which structs
      may name one, so the plan probes that rule before Mojito narrows.
    - Model: Opus, Planned.
  - `iterable-element-identity`: Mojito's `for` yields the *iterable*'s
    `Element`, so one associated type serves a generic signature and the
    loop; the pin yields the *iterator*'s `Iter.Element` and will not convert
    between the two without an identity clause Mojito does not implement.
    - `assets/extensions/ok/iterable_associated_element.mojo` has no `main`:
      its `for` over a trait bound that declares only `Element` never
      compiled upstream, where even a real `Iterable` bound abandons its
      `AnyType` iterator temporary.
    - Model: Opus, Planned.
  - `mojito-only-stdlib-algorithms` / `owning-family-container-apis`: two
    stdlib surfaces upstream does not have — `std.algorithms`,
    `std.collections.string_dict`, and the owning-family container APIs
    (`deinit_with`, `clear_with`, displacement-returning `insert`). Two
    corpus fixtures moved to `assets/extensions/` for them.
    - Whether these leave or stay is a stdlib-shape decision, not a respelling.
    - Model: Opus, Planned.
  - `partial-move-join-imprecision`: a conditional partial move on one
    branch joined with a whole move on the other is accepted, though the
    first path reaches the exit with a hole the pin rejects (`field 'p.a'
    destroyed out of the middle of a value`).
    - The three-point move lattice joins `a: MaybeMoved` under an intact
      base with a wholly moved base into a state it cannot tell from
      intact-or-wholly-moved.
    - Pinned by `conformance/probes/partial_move_join_imprecision.mojo`.
    - A fourth lattice point, or a per-node "may hold a hole" flag that survives
      joins, is the lever; the plan picks one.
    - Model: Opus, Planned.
  - `assign-plain-span-argument-over-list`: `xs = rebuild(Span(xs))` runs
    upstream (`1`) and is rejected in Mojito with "access to 'xs' conflicts
    with live reference 'xs'".
    - The temporary argument's anchor now ends before the store, as for the
      passing `s = String(StringSpan(s))`, but the assigned `List[Int]`
      result still records a loan on `xs`, so the store conflicts with the
      new value itself.
    - Pinned by `conformance/probes/assign_plain_span_argument_over_list.mojo`.
    - Where the result's loan comes from (MIR `aggregate_borrows` or a replayed
      transfer effect) is not yet known.
    - Model: Opus, Planned.
  - `named-tuple-unpack-copy`: unpacking a named Tuple with a heap element
    shares the element with the source, so the VM frees it twice.
    - `var first, second = pair` over `Tuple[Int, String]` prints `3 seven 3`
      at the pin and fails with "use after Pointer deallocation" in Mojito.
    - A `List[Int]` element is rejected at the pin ("cannot be implicitly
      copied"), while Mojito accepts it and fails the same way.
    - The unpack plan in `crates/mojito-checker/src/checker/statements.rs`
      reads each element through the place accessor without the
      implicit-copy check or a copy.
    - Pinned by `conformance/probes/tuple_unpack_named_place_copies.mojo`.
    - The lever is the ImplicitCopy funnel, but the MIR and VM fallout of
      copying at the unpack is not enumerated.
    - Model: Opus, Planned.
  - `ref-field-return-origin-widening`: a view struct that stores its source
    in a direct `ref` field still widens a returned field view to a declared
    `origin_of(self)`.
    - `def view(ref self) -> View[origin_of(self)]` returning
      `View(source, 0)` over `ref source = self.items` passes when `View`
      holds `ref[o] List[Int]`.
    - The `Pointer`-field twin rejects with upstream's "cannot implicitly
      convert 'View[origin_of(self.items)]' value to 'View[origin_of(self)]'"
      (`assets/type_error/return_origin_widening_field_view.mojo`).
    - Why the tail escapes `reconcile_return_origin_tails`
      (`checker/origins/solve.rs`) is not yet known; the likely lever is the
      tail the `ref`-field constructor binds.
    - Pinned by `assets/extensions/ok/ref_field_view_for_temporary.mojo`,
      `ref_field_view_ref_yield.mojo`, `ref_field_view_method_return.mojo`,
      `ref_field_drain_mut_method.mojo`, and `ref_field_chained_view_call.mojo`.
    - The rule already exists; the gap is one constructor path.
    - Model: Opus, Planned.
  - `string-subscript-element`: `s[0]` on a `String` yields a character
    upstream and an `Int` in Mojito
    (`assets/ok/nominal_string_indexing.mojo`, `h` against `104`). Two
    smaller text divergences ride along in the same manifest under
    `one-off`: the raised `DictKeyError` renders differently
    (`assets/ok/self_hosted_dict.mojo`), and reflection prints
    `<unprintable>` upstream where Mojito prints the element types
    (`assets/ok/type_names_applied_elements.mojo`).
    - The subscript's element type is a declaration change in the stdlib's
      `String`, and the other two are texts to match.
    - Model: Fable, Planned.
  - `int-true-division`: `Int / Int` is true division into `Float64` in
    Mojito; the pin truncates back to `Int` and divides only an `IntLiteral`
    pair into a float. An `output-diff` row, not a rejection, so it is not on
    the burn-down.
    - The result type of one operator changes, and every fixture and stdlib body
      that divides integers moves with it.
    - Model: Fable, Planned.
  - `simd-infix-comparison`: Mojito's `<`/`<=`/`>`/`>=` are elementwise at
    every width and its `==`/`!=` compare lane by lane. The pin constrains
    the strict inequalities to `Scalar` and gives `==`/`!=` whole-vector
    meaning, pointing at `SIMD.lt(...)`, which Mojito now has (2026-09-13)
    along with `le`/`gt`/`ge`/`eq`/`ne`. Six corpus fixtures were respelled
    onto the methods.
    - Withdrawing the infix spelling is a leniency to remove, with fallout
      across the stdlib's SIMD bodies.
    - Model: Fable, Planned.
  - `simd-element-narrowing` / `simd-inferred-width` / `float-literal-to-int`:
    three small constructor leniencies. Mojito narrows a SIMD element
    argument to the lane type (wrapping an out-of-range literal and a wider
    runtime value), infers an unbound SIMD width from the argument count, and
    truncates a `FloatLiteral` straight to `Int`; the pin wants the lane's
    own scalar, a written-out width, and the `Float64` it truncates from.
    - Three leniencies to withdraw in one pass.
    - Model: Fable, Not Planned.
  - `float32-reduce-ordering`: `reduce_mul` over `Float32` lanes folds left
    at lane precision in Mojito and pairwise in the pin
    (`319256416.0` against `319256448.0` on sixteen lanes;
    `assets/ok/simd_wide_widths.mojo` now reduces an exactly representable
    vector to avoid it). The float-format entry above hides the same rows.
    - The reduction shape changes in the VM and in the `llvm.vector.reduce.fmul`
      lowering together.
    - Model: Fable, Planned.
  - `string-span-byte-index` / `string-codepoint-index`: `sp[byte=i]` reads
    the byte value in Mojito and returns the one-byte view upstream, and
    `s[codepoint=i]` is a `Codepoint` in Mojito and a one-codepoint
    `StringSpan` upstream. The same shape as `string-subscript-element`
    above, on the keyword subscripts.
    - Declaration changes in the stdlib's `String`/`StringSpan`.
    - Model: Fable, Planned.
  - `split-returns-owned-strings`: `String.split`/`splitlines` return
    `List[String]` in Mojito and owned-interior `StringSlice` views upstream,
    so `var parts = s.split(" ")` then `s = String(parts[0])` runs in Mojito
    and hits upstream's call-result aliasing rejection.
    - Pinned by `conformance/probes/split_returns_owned_strings.mojo`.
    - An API shape change with display and iteration fallout across every
      `split` caller.
    - Model: Fable, Planned.
  - `contiguous-slice-result`: a contiguous `List` slice is an owned `List`
    in Mojito and a borrowing `Span` upstream, so only Mojito returns one
    from a `-> List[Int]` function.
    - The call-result aliasing rule rides on it: upstream rejects
      `xs = rebuild(xs[0:1])` because the slice views `xs`'s owned elements,
      while Mojito's copy borrows nothing and runs. Pinned by
      `conformance/probes/list_slice_copies.mojo`.
    - The return type of `List.__getitem__(ContiguousSlice)` changes, and every
      caller that owns the result moves with it.
    - Model: Fable, Planned.
  - `int-is-floatable`: Mojito conforms `Int` and an integer literal to
    `Floatable`; upstream conforms neither, so a `Floatable`-bounded helper
    takes only a float there. Mojito also resolves `len(x)` from a bare
    `__len__` where the pin wants a declared `Sized` conformance —
    `assets/ok/dunder_index.mojo` and `assets/ok/self_hosted_vec.mojo` now
    declare it.
    - Two conformance leniencies to withdraw.
    - Model: Fable, Not Planned.
  - `slice-descriptor-kinds` / `unmodeled-struct-decorator` /
    `implicitly-deletable-alias`: three spellings the pin has dropped or
    never had. Mojito splits upstream's single `Slice` into
    `ContiguousSlice`/`StridedSlice` and overloads subscripts on the kind, it
    ignores an unmodeled struct decorator where the pin rejects an unknown
    one (`@value` is now unknown there), and it still normalizes
    `ImplicitlyDeletable` to `Deinitable`, which the pin has removed.
    - Each is a name or a type to withdraw, with stdlib and fixture fallout.
    - Model: Fable, Planned.

  Five divergences are retained on purpose and re-probed rather than fixed;
  they are listed in [`docs/non-goals.md`](non-goals.md).

### 4. Grow The CPU Standard Library *(demand-first)*

- [ ] **4.1 Collection API parity**

  Goal: grow the tuple, slice, optional/variant, and String surfaces toward
  the audited head (`docs/features.md` records what lands). The tasks below
  are in impact order: soundness of the executable oracle first, then
  everyday spellings that reject today, then parity details. No task
  depends on a later one. Every bullet is a conscious, recorded limit, a
  task closes when its bullets are done, and a residue found inside a task
  moves to the task that owns its fix.

  1. **Compile-time evaluation residues** — what VM CTFE can bind and
     resolve.
     - A VM-evaluated compile-time expression whose result is
       pointer-backed (`comptime C = M.copy()`, a bare `Optional`, a
       `String`) cannot cross back (`cannot cross back from VM CTFE`).
       Upstream binds it. The freezable results are scalars, Bool, String,
       tuples, fieldwise structs, and displays.
     - Compile-time Dict/Set key identity is structural `CtValue`
       equality. That is exact for every prelude key type, and diverges for
       a user-struct key with a non-fieldwise `__eq__`.
     - The typing probe checks the CTFE subprogram once more per VM-bound
       expression.
     - A string element of a compile-time tuple indexed under a `comptime
       for` (`comptime t = (1, "s")`; `comptime for i in range(2)`) breaks
       inside a list display: `first([t[i], t[i]], t[i])` fails MIR
       verification with `register r19 has no checked type`, and `var v =
       t[i]` followed by `first([v, v], v)` with `binding of StringLiteral
       to a slot of type String`. The `Int`/`Bool` twin runs; the
       materialized element keeps its literal type where the display's
       element type has already materialized `String`. Two `var`
       declarations across unrolled iterations also collide (`'v' is already
       declared in this scope`) unless each body opens a block.

  2. **Monomorphization coverage and cost** — where the erased path still
     stands in for an instance clone, and what minting costs.
     - A generic instantiated at `StringLiteral` by a literal argument
       (`T = StringLiteral`) keeps the erased path, since
       `instance_method_clone_name` mints no clone for it. Displays and
       `StringDict` keys now materialize `String`, so they no longer reach
       it; whether `StringDict.__getitem__` can now return a reference
       instead of its `Copyable`-guarded copy is unchecked.
     - A call inside an unstamped bundled body on a bundled struct's
       generic method keeps the erased path. Requests are admitted from
       user code, clone bodies, variadic specs, instances, and user
       structs.
     - An instance clone whose walk cannot resolve an application (a
       variadic template over a nested public `Tuple` argument) is dropped
       to the erased path rather than failing the program. When such a
       method can reach a compile-time-keyed stub, an instance the
       elaborator queued but could not clone rejects the program instead —
       rejecting one the pin accepts. An instance the checker records no
       instantiation for at all (a type parameter inferred as the
       compile-time `StringLiteral`, `Box("a").f()`) is not queued, so it
       keeps the erased path and aborts at run time.
     - Bundled templates keep their constructors on the erased path: a
       `List`/`Dict`/`Optional` instance builds and copies through the
       template's `__init__`/`__copyinit__`/`__moveinit__`, so a `comptime if
       Self.T` there would not fold. User structs clone theirs, and every
       struct's `__deinit__` clone is reached.
     - An erased body still dispatches by runtime name where no checked
       static type reaches it: an operator or protocol dunder called from
       another erased body, a value whose static type is a bare `Ty::Param`,
       an instance reached only from bundled code, and CTFE, which runs
       before any clone exists. A compile-time-keyed method reached that way
       aborts rather than being rejected.
     - Value-parameterized structs get no instance clones, so an erased
       body's `_unqualified_type_name[Self.T]()` spells `T` (`repr([1, 2])`
       prints `Array[T, 2]([Int(1), Int(2)])` where upstream prints the
       element type). A `Self.n` bracket argument inside such a body
       (`Counter[Self.length](i)`) is VM-only: native monomorphization
       needs a compile-time-constant value argument (`unsupported value
       parameter 'length' is not compile-time constant`).
     - Clones are minted per whole instance with no reachability pruning
       and re-checked each discovery round. `benchmarks/compile/stdlib_heavy`
       is about 2.2x its pre-clone baseline in release
       (`docs/performance.md`), and the repr methods added 2026-09-05 cost
       about 5% in debug across the compile benchmarks. Lever:
       reachability-pruned minting.

  3. **Variadic packs and tuples** — what the pack machinery types
     syntactically or refuses.
     - Type-pack calls inside a nested `def` and whole-pack-forwarded calls
       keep the syntactic element-typing path (`a heterogeneous pack
       specialization needs an expression whose type is statically evident
       before checking` for `take(b, 1)` over a local `b`). Top-level calls
       consult the checker's instantiation. Three of the four pieces this
       needed now exist for ordinary nested generics: an unresolved template
       survives `replace_templates` for the discovery check,
       `def_specialization_requests` harvests a nested callee, and
       `NestedMono::scan_expression` consults that request. What remains is
       the checker accepting a nested variadic shell abstractly, and
       softening the pack diagnostics — only an arity failure defers today,
       and a pack failure is `PackBound` or `NotComptime`.
     - A user variadic struct application as a pack element
       (`Tuple[TypeNames[Int]]`) keeps the fixed-arity diagnostic. Its
       erased shell has no sound nominal form; the public `Tuple` is the
       one compiler-known template whose `*Ts` absorbs every argument.
     - A public Tuple as an explicit type argument does not conform to
       `Deinitable` (`make[T: Defaultable & Deinitable]()` over
       `Tuple[Int, Bool]` reports the bound failure), while the inferred
       shape runs.
     - A standalone nullary vector construction `SIMD[d, w]()` rejects
       (`SIMD construction expects w element(s) or 1 to splat, got 0`).
       Only a Tuple element defaults to zero lanes.
     - `Tuple.reverse` and `Tuple.concat` are typed in Rust from the element
       list (`checker/method_calls/builtin_types.rs`), not declared in
       `std/builtin/tuple.mojo`. The nominal declaration answers a Tuple
       method call first, and this surface serves only what it does not
       declare. Upstream writes both in Mojo over `Self.Ts.reverse()` and
       `TypeList._concat`, which Mojito has no spelling for, so porting them
       waits on type-level pack algebra. Pinned by
       `checker_test::accepts_tuple_constructors_and_structural_operations`.

  4. **Everyday spellings that still reject** — checker context and stdlib
     API shapes.
     - A lambda parameter annotated `String` (`lambda (s: String): print(s)`)
       reports `unknown type 'String'`, while a nested `def` with the same
       parameter runs. The lambda's hidden `def` apparently misses the
       prelude qualification the other annotations get.     - A list display as an argument to an explicitly applied constructor
       at runtime (`Dict[String, Int](["a"], [1], None)`) takes no context
       from the parameterized `List[Self.K]` parameter and materializes as
       `Array`, so no overload matches. Spell the lists
       (`List[String]("a", __list_literal__=None)`), as a compile-time
       value's materialization does.
     - A struct's own value parameter as a SIMD width in a signature
       (`def zeros(self) -> SIMD[DType.int64, Self.length]`) reports `SIMD
       width must be a positive power of two, got a type`. The width slot
       needs a concrete comptime `Int`, where upstream binds the parameter.
     - String literals have no methods (`"abc".byte_length()` rejects). A
       literal-typed value converts through `String(...)` first.
     - An annotated `Optional[T]` local initialized from a bare struct
       construction (`var o: Optional[P] = P(4)`) fails MIR verification
       with an untyped register for the constructor call. Spell the wrapper
       (`Optional[P](P(4))`, `Optional(P(4))`); a literal payload
       (`var n: Optional[Int] = 6`) converts.
     - `if`/`while` accept a width-1 bool lane through the `Bool(x)`
       truthiness conversion, but `and`/`or` still demand `Bool` operands
       (`a == b or c < d` over `UInt64`s rejects). Nest the tests.
     - A string literal does not bind a trait-bounded type parameter the
       nominal `String` satisfies (`isdir("/tmp")` reports `'StringLiteral'
       ... does not conform to trait 'PathLike'`). Spell `String("/tmp")`.
     - Struct-level `comptime NAME = Self(n)` constants (upstream's
       `ErrNo.ENOENT`) do not fold. The associated-constant evaluator
       handles prefix, infix, tuple, and list expressions only.
     - `@fieldwise_init` beside a hand-written `__init__` rejects. Spell
       every constructor by hand.
     - A bound-generic def whose template body calls itself with a concrete
       argument (`makedirs(head, exist_ok=...)` inside `makedirs[PathLike]`)
       mints its own specialization while checking and reports it
       undefined. Recurse through a non-generic helper.
     - Nullary construction of a sized-scalar array (`Array[Int8, 1024]()`)
       reports the scalar's `Defaultable` construction unsupported. Spell
       `Array[Int8, 1024](fill=0)`.
     - `Pointer.unsafe_bitcast[U]()` is not typed: an origin-cast-style
       forwarding leaves the MIR register at the old element type.
     - `CStringSlice` views `Byte` elements instead of upstream's `c_char`.
     - No `std.builtin.rebind.downcast`: `Dict`/`Set` guard `keys`,
       `values`, `items`, and `__iter__` with `where conforms_to(K,
       Copyable)` instead of laundering the parameter. `Dict.keys` needs
       copyable values as well as keys, since the key view wraps the entry
       view.

  5. **Naming and Unicode details** — output text only.
     - `_unqualified_type_name` spells nested structs unqualified where
       upstream keeps a non-prelude struct's module path
       (`Optional[std.collections.dict.Dict[...]]`, `List[up.Flag[True]]`).
       `List`, `Optional`, `String`, and `SIMD` stay bare.
     - Grapheme segmentation is the documented UAX #29 essentials subset:
       hand-maintained Control/Extend/SpacingMark ranges, no
       Extended_Pictographic or Prepend data. Reverse iteration re-scans
       forward from the nearest CR/LF/Control boundary.

- [ ] **4.2 Filesystem and I/O residues**

  Behind the landed files, streams, paths, and tempfile stage
  (`docs/features.md`).

  1. The remaining tail, each a small port:
     - public `stat` / `lstat` / `stat_result`, `realpath`, `symlink` /
       `link` / `chdir`, and `isatty` (`FileDescriptor.isatty` / `fchdir`);
     - `OptionalPointer` (null tests spell `Int(ptr) == 0`);
     - `ErrNo`'s named constants;
     - `~user` expansion (`getpwnam`);
     - the VM's per-process environment overlay (native `setenv` writes the
       real environment);
     - `NamedTemporaryFile`;
     - `FileHandle.read` into a typed `Span[Scalar[dtype], origin]`;
     - `Path.stat` / `lstat` / `_dir_of_current_file`;
     - `KeyElement`;
     - `_get_random_name` over `std.random` (reads `/dev/urandom` today);
     - a generic `__exit__[E]`.
  2. Traps the stage worked around, each its own fix:
     - An overloaded constructor spelled with the `StringSlice` or `Byte`
       alias mangles a different key than the `StringSpan` / `UInt8` the
       call selects. The ports spell the canonical names.
     - A temporary view as a method argument
       (`s.take(String("x").as_bytes())`) is a VM "reference receiver must
       be a place" rejection, and as an augmented-assignment operand
       (`p /= StringSpan(s)`) a VM use-after-free.
     - `Span[mut=True, T, _]` fails origin inference. Spell an
       `[origin: Origin[mut=True]]` binder.
     - The two VM destruction-order divergences the stage found live in the
       behavioral-divergences task of section 3.
     - `range` is invisible in `std.string`.
     - A module loaded while the prelude bootstraps (`std.io` and the whole
       `std.os` graph now) must import `String` explicitly. That graph
       costs Hello World about a second of debug compile time
       (`docs/performance.md`).

- [ ] **4.3 Scalars have no comparison methods**

  Problem: `x.ne(y)` on a `Float64` (or any width-1 scalar) is rejected with
  `type 'Float64' has no method 'ne'`, though the pin accepts it.
  - Upstream's scalars are width-1 `SIMD`, so `lt`/`le`/`gt`/`ge`/`eq`/`ne`
    exist on them too. Mojito resolves those methods only on a multi-lane
    `Ty::Simd` receiver (`crates/mojito-checker/src/checker/method_calls/mc_infer.rs`).
  - The methods must keep the multi-lane semantics: the pin's scalar
    `s.ne(s)` is `False` for a NaN (ordered), while infix `s != s` is `True`.
  - Not a wrong answer, only a missing spelling. Until then, `x < y or x > y`
    is the ordered `ne`; infix `!=` answers `True` for a NaN.

- [ ] **4.4 Time, random, and testing slices**

  Goal: deterministic testable cores, with host-dependent behavior behind
  runtime services.

### 5. Packaging, Artifacts, And Developer Tooling

- [ ] **5.1 The corpus sweeps no longer run in the overnight gate**

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

- [ ] **5.2 Naming the bundled stdlib with `-I` breaks every program**

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

- [ ] **5.3 Compile-time performance**

  Problem: Hello World is 0.8 s release / 4.6 s debug
  (`docs/performance.md`; it was 24 s / 52 s before the shared
  `Arc<CheckedTables>`). Next, in order:
  1. Avoid the redundant checker passes. Hello World re-elaborates and
     re-checks once because the request scan always finds the prelude's own
     Tuple/def requests, and every check runs two transfer rounds.
  2. `checked_var_types` scans the whole expression table per variable, and
     `explicit_destroy` re-derives deinitability per struct per pass.
  3. Only then cache the elaborated/checked stdlib across processes.

- [ ] **5.4 Feature and target options**

  Goal: checked CLI/build configuration recorded in artifacts and
  diagnostics.

- [ ] **5.5 Compiled package artifacts**

  Goal: a versioned `.mojoc` representation (modules stay non-first-class).
  Per-directory resolution order:
  1. source package
  2. `.mojoc`
  3. source module
  4. legacy `.mojopkg`

- [ ] **5.6 Debugging metadata and inspection**

  Goal: stack/source diagnostics, MIR inspection, and debugger-oriented
  value rendering.

- [ ] **5.7 Testing tools**

  Goal: Mojito-native assertions, expected-error tests, and
  differential-harness integration.

- [ ] **5.8 Distribution reproducibility gate** *(last)*

  Goal: the release check rebuilds, tests, documents, and reproduces
  conformance from the crates.io archive alone.

### 6. Code Organization Follow-Ups *(behavior-preserving)*

The 2026-09 module split (`docs/symbol-map.md`) removed every file over
3,000 lines. What remains needs semantic extraction, not line moves.

- [ ] **6.1 Split `expr_unconverted`**

  `mir/lower_expr/expr.rs` (about 2,090 lines) is one match over
  `ExprKind`.
  - Extract arm groups into `Flatten` methods.

- [ ] **6.2 Split `infer_method_call`**

  `checker/method_calls/mc_infer.rs` (about 1,530 lines) is one method.
  - Extract receiver-family branches beside `selection`, `statics`, and
    `builtin_types`.

- [ ] **6.3 Split `verify_instruction`**

  `mir/verify/instr.rs` (about 1,320 lines) is one match over `MirInstr`.
  - Extract per-family check helpers.

- [ ] **6.4 Shrink the 2 kloc band**

  Split these further only along a cohesive seam, while touching them:
  - `checker/traits.rs` (2,629), `mir/lower_stmt.rs` (2,595),
    `checker/inference.rs` (2,520), `checker/statements.rs` (2,471),
    `ast.rs` (2,464), `mir.rs` (2,426), `checker/type_resolution.rs`
    (2,395), `runtime.rs` (2,235), `checker.rs` (2,232),
    `comptime/rewrite.rs` (2,179), `checker/declarations.rs` (2,118),
    `mir/text/write.rs` (2,116), `backend/vm/exec.rs` (2,085).

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
  delete it here and record the outcome in `docs/features.md`,
  `CHANGELOG.md`, and (for design invariants) `docs/architecture.md`.
- Rewrite partially completed tasks so only the remaining outcome is stated;
  prefer one checkbox per independently demonstrable outcome.

## Entry Style

Every entry is written for a human reader who has not seen the code.

- One problem per checkbox. Its first sentence states the issue to fix. An
  item we are not going to fix does not belong here at all — it belongs in
  [`docs/non-goals.md`](non-goals.md).
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
- Entries are numbered `<section>.<n>` and sorted by dependency first, then by
  how much the entry moves that section's goal. An entry that depends on
  another names it. Renumber when one lands.
- After renumbering, recheck every number and every reference to one. Each
  number appears once in its section, and each "Depends on" names the entry
  it meant, never the entry that carries it.
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
