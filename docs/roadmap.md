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
box is the default next task. Every entry carries a **Depends on** bullet
naming the entries it needs, or `nothing` — an entry that merely makes another
one easier counts as a dependency. The **Model:** bullet is an estimate, not a
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
to section 3, however small. The four entries that measure or rehearse the
Pliron experiment rather than move the check order sit last, 1.15 to 1.18.

- [ ] **1.1 An overloaded witness beside a rival the recorded types cannot
  rank keeps the clone check**

  Problem: a call through a bound whose witness overloads the requirement
  derives only when every member of the arity can be ranked on the
  recorded argument types (`template_facts/bound_dispatch.rs:member_rank`).
  - A rival with a reference-typed parameter is ranked by the argument's
    place and origin (`infer_storage_value`), which no recorded type holds.
  - A variadic rival ranks each collected argument's literal-ness and
    by-value binding, which the recorded type does not tell.
  - A collection or tuple display, a leading-dot member, or an explicit
    application as the argument takes its type from each rival's
    parameter, so only the argument expression ranks it.
  - A rival's binder beyond a bare parameter type (`List[U]`, `Scalar[dt]`)
    is inferred only over closed argument types; over a caller binder the
    instance keeps, it still refuses (`member_parameters`).
  - No bundled type overloads a requirement, so `stdlib_heavy` is unmoved.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.2 A field handed to a hasher the instance binds to a struct
  keeps the clone check**

  Problem: `hasher.update(self.value)` in a method whose own `H: Hasher`
  an instance binds to `AHasher` refuses the derivation.
  - `realize_nominal_hasher_call` borrows a read argument where it lies
    only when `expression_bindings` names its place, and a field read
    through `self` has no entry there.
  - It then cannot tell the field from the receiver, so it refuses as for
    an overlapping place ("a hasher argument's place is unbound or overlaps
    the receiver").
  - A temporary argument (a `SIMD` construction) already derives.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.3 A string literal or other non-scalar value handed through a
  bound keeps the clone check**

  Problem: `self.s.scale(2, label="y")` through `S: Scaler` refuses the
  template, since `BodyShape::bound_dispatch` admits only a closed scalar
  or a named place as an argument.
  - A named local of a closed non-scalar type is refused the same way:
    `kept_argument` asks a by-value argument to be a scalar.
  - A default left out, and a keyword argument, derive
    (`assets/ok/template_method_bound_defaulted_argument.mojo`).
  - A string requirement default a witness declares otherwise is spelled
    at the call (`checker/bound_defaults.rs`), so it keeps the clone check
    too.
  - Found while defaults through a bound joined the method grammar
    (2026-09-28); no bundled body is known to need it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.4 `Tuple`'s default initializer has no symbolic template**

  Problem: each Defaultable `Tuple` specialization still infers its
  synthesized `__init__(out self)` once, because nothing checks that
  initializer with the pack symbolic.
  - Hello World infers 4 such bodies, one per specialization; every later
    discovery round derives from that first copy
    (`FirstCopy::OwnSpecialization`).
  - The bundled `Tuple` declares no default initializer: the specializer
    writes one per specialization (`tuple_default_constructor`).
  - Upstream spells it in source over the symbolic pack:
    `comptime for i in range(Self.__len__()):
    Pointer(to=self[i]).unsafe_write({})`.
  - A pack element's construction now checks symbolically: `Self.Ts[i]()`
    and a `def`'s `Ts[i]()` are the dependent element, and each instance
    elaborates the element's own construction
    (`infer_pack_element_construction`, `pack_element_construction`).
  - Mojito has no spelling that builds `__RuntimeTuple` storage element by
    element, as upstream's `mark_initialized` and `unsafe_write` do.
  - No recipe derives an elaborated element construction. Its facts differ
    by element kind: `Int()`, a SIMD scalar such as `UInt64`, `String()`, a
    struct such as `Optional[Int]` with its overload target, a nested
    `Tuple` instance.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **1.5 `repr` in a runtime `def` keeps the clone check**

  Problem: `var r = repr(kept)` in `def shown[T: Writable & ...](x: T)`
  refuses the template, though the same call in a generic struct's method
  derives.
  - The function certificate refuses any body that recorded a `repr` call
    (`repr_calls`), before it looks at the features the body holds.
  - `STRING_BUILTINS` is already one of `FUNCTION_FEATURES`, for a `print`
    of a whole value (`assets/ok/template_print_whole_value.mojo`).
  - `_unqualified_type_name[T]()` in such a `def` now passes the class and
    is refused at the trace ("the clone's occurrences are not the
    template's").
  - Admitting them needs the function path to realize `repr_calls` and the
    `TypeName` adjustment as the method path does.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.6 A nested `def` with a constructed default keeps the clone
  check**

  Problem: `BodyShape::nested_def` admits only a closed-scalar,
  string-literal, or `None` default.
  - `def join(x: Int, sep: String = String("-"))` keeps the clone check, as
    the test `template_nested_def_constructed_default_keeps_the_clone_check`
    pins.
  - A call leaving the argument out runs on the VM, which runs such a
    default as a lowered default function (`CheckedConst::Evaluate`).
  - Found while literal defaults, typed `raises`, and `out` parameters
    joined nested `def`s in the method grammar (2026-09-28); no bundled body
    is known to need it.
  - A module `def` with such a default refuses the same way: the function
    class admits only a literal or negated numeric default
    (`template_facts.rs:literal_default`, 2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.7 An explicitly applied or overloaded generic static with
  binders of its own keeps the clone check**

  Problem: `Pair[Self.T].scaled[3](2)` refuses the template, and so does a
  call to an overloaded static one of whose members declares binders.
  - The explicit application is an `Invoke` that `BodyShape::
    parameterized_call` admits only on a value receiver, not a type.
  - An overloaded family with binders would need the per-call clone of
    the member the template ranked; `realize_static_instantiations` handles
    a lone static only.
  - Before the per-call clone exists (round 0), the instance calls its own
    clone of the static, which no recipe repeats, so such a call derives
    only from round 1.
  - Found while own binders joined generic statics in the method grammar
    (2026-09-28); no bundled body is known to need it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.8 A field read of a sibling call's result keeps the clone
  check**

  Problem: `return self.bumped().count`, a field of a sibling method's
  whole-value result, sits outside the method grammar.
  - `Bag.twice` in `assets/ok/template_method_variadic_struct.mojo` is
    checked again per specialization for this alone.
  - A sibling call standing alone, and `len(self)`, derive.
  - Found while `Tuple(*args^)` initializers joined the grammar
    (2026-09-28); no bundled body is known to need it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.9 A nested t-string's `write_to` is checked again in an early
  discovery round**

  Problem: before the outer t-string's storage `Tuple` is declared, the
  instance finds no accessor for a position (`realize_pack_accessors`), so
  that round runs the clone check.
  - The final round derives the same member; only the early round pays.
  - A plain `Tuple` element read refuses the same way before its `Tuple`
    is declared (`realize_tuple_elements`).
  - Found with `assets/ok/tstring_forms.mojo` (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.10 A float lane's `__fma__` over a temporary argument keeps the
  clone check**

  Problem: `BodyShape::lane_float_method` admits `k.__fma__(a, b)` on
  values of a symbolic lane only when each argument is a named place.
  - At the native `Float64` the call resolves as that scalar's method,
    which borrows a place argument but records a read temporary for any
    other.
  - `realize_lane_float_methods` realizes only the borrows.
  - Found while the float range's members joined the grammar (2026-09-28);
    no bundled body passes a temporary.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.11 A lane comparison's mask used as a value keeps the clone
  check**

  Problem: `var m = self.pos == other`, or a comparison of a symbolic lane's
  value with a literal (`x < 0`), is refused by the method grammar, so the
  body is inferred per instance.
  - A lane comparison derives only as a condition or as `Bool(...)`'s
    argument (`BodyShape::lane_comparison`), where re-typing it per lane
    changes nothing else.
  - Bound to a local, the mask's binding and every read of it become a
    `Bool` where the lane folds to `Int` or `Float64`; a recipe would
    re-type each.
  - A literal operand materializes at the lane's type, which
    `realize_lane_literals` does for arithmetic but not for a comparison.
  - Found while lane comparisons joined the grammar (2026-09-28); no
    bundled body is known to need it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.12 A `while` loop in a compile-time-keyed `def` keeps the clone
  check**

  Problem: a keyed `def` holding any `while` loop, even `while n < 3:` over
  an `Int` local, is refused by the method grammar, so each instance is
  inferred.
  - The grammar admits `while` only outside a keyed body
    (`BodyShape::statement`); `if` and `elif` derive there.
  - Found while probing lane comparisons in keyed `def`s (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.13 `Array`'s copy and move initializers, `deinit_with`, and
  owned `__iter__` keep the clone check**

  Problem: these four `Array` members are still inferred again in every
  checker pass, while the rest of `Array` reuses its templates.
  - The copy and move initializers are lifecycle initializers, whose
    `copy:` and `deinit move:` receivers the method class refuses outright
    (`method_certificate`).
  - `deinit_with` hands each taken pointee to its callable parameter, and
    reuse refuses it: "a substituted value may carry a loan where the body
    keeps a residue".
  - The owned `__iter__` constructs `_ArrayOwnedIter[Self.T]`, and reuse
    refuses it: "a constructed struct has a parameter that is not a plain
    type".
  - Found while landing the rest of `Array` (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.14 Keyword arguments out of parameter order through a bound
  derive facts the clone check does not match**

  Problem: `self.s.shift(1, extra=a, by=b)` through `S: Scaler` fails
  template-fact verification (`MOJITO_VERIFY_TEMPLATE_FACTS=1`).
  - The derived call boundary lists the arguments in parameter order; the
    clone's own check lists them in source order.
  - The program runs the same either way: the arguments still evaluate in
    source order.
  - A requirement default spelled at the call goes before the first
    keyword for a later parameter, so it does not trip this
    (`checker/bound_defaults.rs`).
  - Found while binding requirement defaults at the call (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **1.15 The A1 shadow core has never been run over the decision
  corpus**

  Problem: the shadow `mojito_core` dialect converts the ten focused inputs,
  and nobody knows how many of the 848 inputs of Stage A1's decision corpus
  it refuses, so the bulk measurement lane cannot run.
  - The record is [`docs/notes/pliron-a1.md`](notes/pliron-a1.md) §Coverage.
  - 25 MIR forms are still rejected by name. Variants, closures, slices,
    tuple construction, and uninitialized storage are among them.
  - A `return` inside a `try` region, a floating constant or default, and a
    call with captured-owner effects are refused by name too.
  - Run `examples/pliron_a1.rs` over `target/pliron-a1/corpus-inputs.tsv`
    first. A refused input ends its diagnostic with every refusal, counted
    (`import::refusals`).
  - The inventory is closed on purpose: each new form needs an operation, a
    verifier rule, an import and export rule, and positive and negative
    cases (`inventory::CoreOpKind`).
  - This entry and 1.16 to 1.18 are the ones in this section that do
    not move the check order. They shadow MIR below `CheckedProgram`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **1.16 The A1 adapter has never been rehearsed against a newer
  Pliron**

  Problem: the pivot's maintenance model assumes an upgrade stays inside the
  adapter, and nothing has tested that since the A1 slice landed.
  - Rehearse one recorded upstream SHA in a disposable copy of the checkout.
  - Record the hours, the changed files, and any change that escapes
    `crates/mojito-pliron` and its feature-gated tests.
  - The budget is one working day. A needed fork rejects the model.
  - The 0.17 to `477e6b0` upgrade in `docs/notes/pliron-promotion.md` is
    historical evidence, not this rehearsal.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **1.17 A1 core text is about three times the v1 text**

  Problem: the canonical text of a core module is 2.6 to 3.4 times the v1
  text of the same module, over the 2.0 line that makes a design review
  mandatory.
  - The gate is 199,537 bytes of core for 69,504 of v1, and `stdlib_heavy`
    is 6,558,137 for 2,092,497.
  - Identity and provenance attributes repeat the function symbol on every
    operation (`a1/attrs.rs`, `IdentityAttr`, `ProvenanceAttr`).
  - Text also dominates the boundary's time: printing and parsing are 780 of
    870 ms in the debug diagnostic.
  - A custom operation format or a per-function symbol table are the levers.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **1.18 The Pliron pivot's overhead has never been measured**

  Problem: the A1 slice passes four of the plan's five falsifiers, and the
  fifth, overhead, has no release measurement, so the pivot is neither
  approved nor rejected.
  - The record and the verdict table are in
    [`docs/notes/pliron-a1.md`](notes/pliron-a1.md).
  - The budget is a paired compile-time ratio strictly below 1.19 and a
    peak-memory ratio strictly below 1.30, each with three median absolute
    deviations of margin.
  - `scripts/bench-pliron-a1` collects the samples. The note lists the
    commands.
  - The bulk run is its own lane: ten pairs over 848 inputs, alone, on a
    quiet machine.
  - Decide from the measurements: plan A2, or record the rejection in
    [`docs/non-goals.md`](non-goals.md) and remove the experiment as the
    note's §Removal lists.
  - This is the decision point for MIR-as-a-dialect, not a commitment to it.
  - The focused lane needs nothing more: all ten focused inputs convert.
  - Depends on 1.15, 1.16, and 1.17.
  - Model: Fable, Planned.

### 2. Native Backend

The ABI-bump collector is last whatever else moves, because it batches every
change that needs a new `MJRT_ABI_VERSION`.

- [ ] **2.1 A generic struct's static taking a pack of its parameter type
  does not compile natively**

  Problem: `Pair[Self.T].count(self.item, self.item)`, beside
  `def count(*values: Self.T)`, prints `2 2` on the VM and the pin, but
  the native backend stops with "unsupported unresolved type parameter
  `T`" in `Pair.count`.
  - The erased static reaches native lowering with its pack element still
    the struct's symbolic parameter.
  - A pack of a closed type (`*values: String`) compiles natively
    (`assets/ok/template_method_generic_static_shapes.mojo`).
  - Probe: `conformance/probes/static_parameter_pack_native.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **2.2 A compile-time `Float64` printed at run time does not compile
  natively**

  Problem: `comptime x = Float64(2.5)` then `print(x)` prints `2.5` on the
  VM and the pin, but the native backend stops with "unsupported display
  of a runtime FloatLiteral value".
  - The folded constant reaches native lowering as a `FloatLiteral`
    register, which `print` has no display for.
  - `comptime z = Int()` prints natively
    (`assets/ok/scalar_default_construction.mojo`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **2.3 An evaluated default argument does not compile natively**

  Problem: `f()`, beside `def f(s: String = String("a"))`, prints `a` on
  the VM and the pin, but the native backend stops with "unsupported
  evaluated default argument of `f` is not yet lowered natively".
  - MIR lowers such a default as the zero-parameter function
    `$default$<owner>$<parameter>` and records it as
    `CheckedConst::Evaluate`, which the VM runs at the call.
  - Native lowering fills an omitted slot only from a literal or a
    converting construction of one (`default_argument_value` in
    `lower/calls.rs`); it needs to call the default function, and
    reachability (`lib.rs`) needs the edge no call instruction spells.
  - Probe: `conformance/probes/constructed_default_argument.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **2.4 Native runtime ABI bump: land every change that needs a new
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

Sorted by dependency first: an entry another one needs comes before it,
however small, and a cluster sits where its most important member would.
Otherwise sorted by how far the entry is from the pin's answer: a program
Mojito runs to a wrong result or accepts where the pin rejects comes first,
then one it rejects where the pin runs it, then a verdict that is right with
the wrong words. The representation gap and the two standing ledgers are
last.

- [ ] **3.1 A reference returned through an origin binder outlives its argument**

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
  - A module `def`'s reference result no longer shows it: MIR reads it out
    of a hidden handle slot that loans the places its origin names
    (`expr_with_adjustments`). A method's result read could take the same
    path once its binder resolves to the argument.
  - The plan must say where a call's inferred origin arguments become loans on
    its result.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.2 Shift operators bind looser than the bitwise ones**

  Problem: `a << 1 | b >> 1` parses as `(a << 1 | b) >> 1`, where Python and
  Mojo bind `<<`/`>>` tighter than `&`, `^`, and `|`, so it prints `7` for
  `Int32(6)` and `Int32(3)` where the pin prints `13`.
  - Found by `assets/ok/simd_symbolic_surface.mojo`, whose `guarded`
    parenthesizes around it.
  - The lever is the infix precedence table in `crates/mojito-parser`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.3 A local passed into a read type pack is destroyed before the call**

  Problem: `show(y, 5)` against `show[*Ts: Writable](*a: *Ts)` runs
  `y.__del__` before `show`'s body, which then prints `y` anyway, where the
  pin destroys `y` after the call returns. It is a silent wrong order.
  - A top-level `def` and a method collector behave alike, on the VM and
    natively; a direct `print(y, 5)` is correct.
  - The call's last use of `y` is its pack argument, so the drop is placed
    as if the collector had consumed it.
  - Probe: `conformance/probes/pack_argument_destroyed_before_call.mojo`.
  - The plan must say whether the ownership analysis or drop elaboration
    misreads the collector's argument convention.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.4 An owned pack's elements are destroyed after the first element's
  last use**

  Problem: `take(Noisy(1), Noisy(2))` against `take[*Ts](var *a: *Ts)`,
  printing each `a[i]` in a `comptime for`, destroys both elements after
  printing the first, where the pin prints both and then destroys them in
  reverse order. It is a silent wrong order.
  - A top-level `def` and a method collector behave alike.
  - The unrolled `a[0]` read is taken as the whole pack's last use.
  - Probe: `conformance/probes/owned_pack_elements_destroyed_early.mojo`.
  - Depends on 3.3, which settles what a pack argument's last use is on the
    caller's side.
  - Model: Opus, Not Planned.

- [ ] **3.5 A constructor overload set drops every generic candidate before it is
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
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.6 A view returned by a method on a `List` element reads freed memory on
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
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.7 Unpacking a tuple with a `String` element frees the element twice**

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
  - Depends on nothing. The `named-tuple-unpack-copy` divergence in 3.83 is
    the same unpack plan and closes with it.
  - Model: Opus, Planned.

- [ ] **3.8 A `^` transfer of a read parameter is accepted and frees the
  caller's value**

  Problem: `def ident(x: String) -> String: return x^` runs in Mojito and
  fails with "use after Pointer deallocation" once the caller's `String` is
  read again or dropped, while a read parameter cannot be transferred in
  Mojo.
  - The same body over a bare type parameter (`def ident[T: Movable](x: T)
    -> T`) fails the same way for a `String` argument and prints for an
    `Int` one.
  - Pinned by `conformance/probes/read_parameter_transfer_returned.mojo`;
    the pin's diagnostic is still to be observed by the sweep.
  - The lever is the checker's transfer check on a parameter with no `var`
    convention, which today admits the move and lets the callee's drop
    free the caller's storage.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.9 A static call copies a place into a `var` parameter whatever its
  type**

  Problem: `Box.keep(self.item)` with `keep(var v: Self.T)` runs on Mojito
  for a `String` instance and stops with "double free of Pointer
  allocation". The pin rejects it ("value of type 'T' cannot be implicitly
  copied").
  - A method call's `var` argument demands `ImplicitlyCopyable` of a copied
    place; the static path (`finish_static_call`) does not.
  - Pinned by `conformance/probes/static_var_parameter_implicit_copy.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.10 A user `Hasher` cannot spell `update` the way the pin requires**

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
    checker's builtin hashable-leaf arm (`record_hash_leaf`), so the call
    side the rename needs exists; `Hasher.update` itself is still the
    checker's intrinsic, not a real `SIMD.__hash__` body.
  - Upstream also replaced the pointer-and-length `hash()` overload with
    `hash_bytes(ImmSpan[Byte])`, which lands in the same pass.
  - The levers are `checker/traits.rs` (the `Hasher` requirement set and its
    shape message), `checker/method_calls/mc_infer.rs` (the intrinsic arms),
    and `stdlib/std/hashlib/` (`hasher.mojo`, `_ahash.mojo`, `_fnv1a.mojo`
    plus every `hasher.update(...)` call site).
  - It changes a compiler-known trait's contract.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **3.11 Forwarding a named accessor's reference result is rejected**

  Problem: `return self.items.unsafe_get(index)` under
  `ref[origin_of(self.items)._get_owned_interior["element"]]` reports
  "returned reference escapes storage outside its declared origin", where the
  pin runs it.
  - The subscript `return self.items[index]` under the same origin works.
  - Only the subscript path records the interior generation the return check
    compares (`indexing.rs`, `origins/interior.rs:record_interior_reference`).
    A named call leaves the returned reference's place without it.
  - Pinned by `conformance/probes/reference_return_forwarded_accessor.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.12 A returned reference may declare a wider origin than its place**

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
  - Depends on 3.11: the bundled accessors this respelling touches forward
    their result through a named call, which is rejected until that lands.
  - Model: Opus, Planned.

- [ ] **3.13 A store to a field of `self` under a live view of it is accepted**

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
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.14 A place passed to a `ref` parameter may alias a pack element**

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
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.15 A `var` of an opaque type is not required to be `Deinitable`**

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

- [ ] **3.16 A comprehension element may transfer its owned binder**

  Problem: `[x^ for x in items^]` runs on Mojito, where the pin reports
  "expression does not designate a value with an origin" at `x^`.
  - The pin takes the bare binder (`[x for x in items^]`), which both run.
  - Mojito's comprehension check consumes the element as a whole value
    (`check_consuming` in `check_comprehension`), and a `^` on an owned
    binder passes as the move of a local.
  - Pinned by `conformance/probes/comprehension_binder_transfer.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.17 Owned iteration of a variadic pack moves elements the pin copies**

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

- [ ] **3.18 A module-level `def` converts to a `def(...) capturing[_]` runtime
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

- [ ] **3.19 `@explicit_destroy` is accepted on a type that is `Deinitable`
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

- [ ] **3.20 A subscript store on a struct with a setter and no getter is
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
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.22 A trivial value handed to a `var` parameter beside a pack is always
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

- [ ] **3.23 A trivial rvalue handed to a `var` parameter beside a read overload is
  accepted**

  Problem: `q(x + 1)` against `q(var a: Int)` beside `q(a: Int)` is "ambiguous
  call to 'q'" at the pin, while Mojito selects the `var` overload.
  - The pin accepts the literal `q(7)`, the place `q(x)` and the transfer
    `q(x^)` in the same program, so only the non-literal trivial rvalue is
    ambiguous. No rule was recovered.
  - This is the non-variadic sibling of 3.22, and the two
    disagree: beside a pack the pin calls a bare literal ambiguous, here it
    accepts one.
  - `ArgumentBinding`'s `undecided` bit models the pack rule and is deliberately
    kept off non-variadic candidates, since applying it would reject `q(7)`.
  - Pinned by `conformance/probes/overload_var_trivial_rvalue.mojo`.
  - The plan must find the rule with more probes, or move the entry to
    `docs/non-goals.md` as a kept divergence.
  - Depends on 3.22, where the `undecided` rule is modelled: this one reuses
    the rule that entry recovers, or is withdrawn with it.
  - Model: Opus, Planned.

- [ ] **3.24 A list literal beside a `List` parameter and a pack is not ambiguous**

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

- [ ] **3.25 Each unrolled `comptime for` iteration shares the enclosing scope**

  Problem: `comptime for i in range(2):` with `var v: Int = i` in its body
  prints `0` then `1` at the pin, while Mojito reports "'v' is already
  declared in this scope".
  - The elaborator splices every unrolled copy of the body into the enclosing
    block (`comptime/elab.rs`, the `ComptimeFor` arm), so the second copy
    redeclares the first's locals; the validator checks the body once in its
    own scope and accepts it.
  - A `comptime n = names[i]` binding in the body fails the same way.
  - Pinned by `conformance/probes/comptime_for_body_scope.mojo`.
  - Each iteration needs its own scope, or its locals renamed.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.26 A reflection method call in a runtime position is rejected**

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
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.27 A field name under a symbolic index prints without `materialize`**

  Problem: `print(names[i])` inside `comptime for i in range(len(names))`
  over `comptime names = reflect[T].field_names()` prints at Mojito, while
  the pin rejects it with "cannot materialize comptime value of type
  'Array[StringSpan[...]]'" and needs `materialize[names[i]]()`.
  - Mojito accepts a program the pin rejects, so this is a divergence.
  - `materialize[names[i]]()` fails at the instance in turn ("Undefined
    variable 'materialize'"): the elaborator's crossing takes a bare binding
    only.
  - Depends on 3.25 for the `comptime n = names[i]` workaround, and on 3.26
    for which handle results may materialize at all.
  - Model: Opus, Planned.

- [ ] **3.28 `repr` of a sized scalar omits or misstates its type name**

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
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.29 `hash` of a struct that overloads `__hash__` stops at run time**

  Problem: `hash(Twin(1))` for a struct declaring `__hash__` for `Some[Hasher]`
  and for a concrete `AHasher` passes the checker and then fails with "vm:
  unknown method 'Twin.__hash__'", where the pin prints the hash.
  - The call reaches the VM under the method's plain name, which no member
    of the overload set is lowered under.
  - Found while probing overloaded witnesses for a bound dispatch.
  - Pinned by `conformance/probes/overloaded_hash_through_hash.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.30 A bound call's result converted at an annotated binding fails
  MIR verification**

  Problem: `var x: Optional[T] = v.copy()` in a generic `def` reports
  "register r1 has no checked type" for the `copy` call, where the pin runs
  it.
  - The same binding at a concrete type runs, and so does the result bound to
    an unannotated local first and then converted.
  - Probe: `conformance/probes/bound_call_result_converted_at_binding.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.31 Two type-pack `__init__` overloads cannot be constructed**

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
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.32 A tuple binding with a `Tuple[...]` annotation loses `reverse` and
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
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.33 A generic struct's `Tuple` field over its parameter cannot be read**

  Problem: a `Holder[T]` field `var pair: Tuple[Self.T, Int]` stops at
  `some_int.pair[1]` with "cannot index Tuple$t2[y3:Inty3:Int]", where the
  pin prints the element.
  - A non-generic struct's `Tuple[Int, Int]` field indexes cleanly, and
    the checker accepts both.
  - The message is the VM's place projection (`backend/vm/places.rs`).
  - Pinned by `conformance/probes/generic_struct_tuple_field.mojo`.
  - Depends on 3.32, which decides whether such a field keeps the public
    `Tuple` spelling the VM can project.
  - Model: Opus, Not Planned.

- [ ] **3.34 A `String` element copied out of a `Span` fails**

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

- [ ] **3.35 A generic method moving its `var` parameter into a sibling call's
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
  - Depends on nothing. The `moved-parameter-into-local-collection`
    divergence in 3.83 is the same stand-in place and closes with it.
  - Model: Opus, Not Planned.

- [ ] **3.36 A value-returning body that ends in `abort(...)` is rejected**

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

- [ ] **3.37 A trait default body holding a `comptime if` never elaborates**

  Problem: a trait whose default method holds `comptime if True:` reports
  "unsupported feature: comptime if" for every conformer, where the pin
  checks the default symbolically and runs it.
  - The elaborator specializes struct methods and `def`s; a default body a
    conformer inherits is never stubbed or selected, so the construct reaches
    the executable check.
  - The upstream reflective defaults (`Hashable.__hash__`, `Equatable.__eq__`)
    are written as such bodies over `reflect[Self]`.
  - The plan says where an inherited default is copied into a conformer today
    and where its selection would run.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.38 A binder is not inferred through a converting argument**

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

- [ ] **3.39 A list literal does not reach a `List` built over a binder**

  Problem: `count([1, 2])` for `def count[T: …](extra: List[T])` reports
  "cannot infer type parameter 'T' of 'count' from the arguments", and
  `Bag[Int]([1, 2])` for a fieldwise `var extra: List[Self.T]` reports
  "type mismatch for field 1 of 'Bag': expected List[Int], found List[T]";
  the pin runs both.
  - The literal is typed at the declared `List[T]` without its binder solved
    or substituted, so the field case fails even with `T` given explicitly.
  - Pinned by `conformance/probes/list_literal_to_generic_list.mojo`.
  - Found while deriving another struct's overloaded method; not root-caused.
  - Depends on 3.38, which lets a binder be solved through a conversion; the
    literal's element type reaches `T` the same way.
  - Model: Opus, Not Planned.

- [ ] **3.40 A literal passed to a `ref` parameter stops at run time**

  Problem: `look(3)` against `def look(ref other: Int)` is accepted and then
  fails with "reference binding to a non-place expression".
  - The pin materializes the literal and binds the parameter to the temporary.
  - Probe: `conformance/probes/literal_to_ref_parameter.mojo`.
  - Depends on nothing. It materializes through the same fallback the
    `ref-binding-register-value` divergence in 3.83 wants narrowed, so the
    two answers must agree.
  - Model: Opus, Planned.

- [ ] **3.41 A call through a `ref` to a callable value is rejected**

  Problem: `for f in fns: print(f(5))` and `ref g = fns[0]; print(g(1))` run at
  the pin over a function display, while Mojito reports "'f' has type ref
  def(Int) thin -> Int and is not callable".
  - Both thin and capturing elements are affected.
  - The indexed call `fns[0](5)` already works through element-call dispatch.
    A `ref`-typed callee has no such path.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.42 A method called on a borrowed comprehension binder loses its
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

- [ ] **3.43 Constructing a struct's own type parameter fails in every instance
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
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.44 `Self(...)` does not construct inside a method**

  Problem: `return Self(self.a, not self.b)` in a method of an
  `@fieldwise_init` struct reports "Undefined variable 'Self'", and so does
  the keyword form; the pin constructs the enclosing struct.
  - Naming the struct (`P(...)`, `G(...)` in a generic struct) constructs,
    by position or keyword, so the gap is resolving `Self` as a callee.
  - Pinned by `conformance/probes/self_call_construction.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.45 Explicit type arguments on a static method of a non-parametric
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
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.46 An associated alias is not constructible through a parameterized
  base**

  Problem: `Holder[7].Same()` for `comptime Same = Sized[Self.n]` reports
  `Undefined variable 'Holder'`, where the pin runs it.
  - The annotation spelling works: `var made: Holder[7].Same = Sized[7]()`.
  - The alias now binds the instance's value parameters
    (`associated_type_from_base`), so only the call path is missing.
  - Depends on 3.45, which finds the pass that reads a bare type name as a
    value; the alias call needs the same answer.
  - Model: Opus, Planned.

- [ ] **3.47 A generic `def` cannot spell a static call's receiver with its
  own binder**

  Problem: `return Pair[T].keep(x.copy())` in `def make[T: ...](x: T)` fails
  with "unknown type 'T'", where the pin runs it.
  - The same spelling in a generic struct's method, `Pair[Self.T].keep(...)`,
    runs on both.
  - Pinned by `conformance/probes/generic_def_static_receiver_binder.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.48 A leading-dot static call cannot take its struct's parameters
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

- [ ] **3.49 A `def`'s own type pack cannot be queried in a runtime position**

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
  - `def` specialization binds its packs for one fold only:
    `fold_pack_element_constructions` elaborates `Ts[i]()`.
  - Pinned by `conformance/fixtures/pack_length_runtime_position.mojo`
    (`pack-length-runtime-position`, `mojo-only`).
  - It stays in the elaborator, so it does not wait for section 1's symbolic
    pack work.
  - The binding site is known, but the plan must enumerate the clone paths that
    share it (free `def`, method-own pack, nested forwarding) and the
    materialization rewrite each one runs.
  - Depends on nothing, and in particular not on section 1's symbolic pack
    work, since the binding stays in the elaborator.
  - Model: Opus, Planned.

- [ ] **3.50 An exact constructor overload loses to its generic sibling as
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
  - Depends on 3.5, which teaches constructor selection to tell a clone from
    a declared overload. Ranking by specificity means nothing while a
    candidate is dropped before it is ranked.
  - Model: Opus, Planned.

- [ ] **3.51 Methods cannot overload on the parameter convention alone**

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

- [ ] **3.52 Upstream `DType` names with no Mojito dtype are rejected**

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
  - Depends on nothing. The bundled `struct DType` port in 3.82 rewrites the
    same table, so this lands first or folds into it.
  - Model: Opus, Planned.

- [ ] **3.53 Small SIMD surface gaps the symbolic-lane probes found on concrete
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

- [ ] **3.54 A `mut self` witness does not conform to a read-`self`
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

- [ ] **3.55 An imported alias in a generic struct's method signature is an
  unknown type**

  Problem: `def feed(self, mut hasher: default_hasher)` on a generic struct
  reports "unknown type '__module$hasher$default_hasher'" for its instance,
  while the pin runs it.
  - The same annotation resolves on a module-level `def`, on a plain
    struct's method, and through a local `comptime` alias of the import.
  - Workaround: spell the application (`AHasher[SIMD[DType.uint64, 4](0)]`),
    as `assets/ok/template_method_bound_witness_shapes.mojo` does.
  - Pinned by `conformance/probes/imported_alias_in_generic_method.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.56 An uncalled `rebind` method is checked for every instance**

  Problem: a method whose `rebind` does not hold for one instance of its
  struct (`rebind[Int](self.value)` on `Box[String]`) is refused with "type
  mismatch for rebind" even when no call reaches it; the pin judges only the
  instances a call reaches and runs the program.
  - The elaborator clones every method of an instantiated struct, and each
    clone discharges its `rebind` equality.
  - Under `comptime if Self.T == Int` the arm is dropped for `String`, so a
    keyed `rebind` is unaffected.
  - Pinned by `conformance/probes/rebind_method_uncalled_instance.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.57 A value-keyed `def` cannot forward its value to a keyed `def`**

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

- [ ] **3.58 A compile-time-keyed `def` cannot be passed as a function value**

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

- [ ] **3.59 A member-led arithmetic type argument does not parse in an alias
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

- [ ] **3.60 An arithmetic `where` operand compiles for `def`s and struct methods
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

- [ ] **3.61 `comptime assert` is not parsed**

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

- [ ] **3.62 `reflect[T]` over a function's type parameter is rejected**

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

- [ ] **3.63 `reflect[T]` of a non-struct type is rejected**

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

- [ ] **3.64 A field's value cannot be read by reflection**

  Problem: `reflect[T].field_ref[i](x)` — the value of field `i` of `x` — is
  unsupported; the upstream hashing, equality, and writing defaults are
  written with it.
  - Mojito's defaults are Rust AST synthesis (`comptime/synth.rs`) and a
    lowering (`mojito-pliron/src/lower/print.rs`), so no bundled body needs
    it yet.
  - Under validation the result's type is the opaque field type
    (`types[i]`), which the arm-licensing rule already covers.
  - Depends on 3.61 for the upstream spelling of the defaults, and on 3.37,
    the body shape they are written in.
  - Model: Fable, Planned.

- [ ] **3.65 A local compile-time type list is unknown in an instance's annotation**

  Problem: `var v: types[i] = ...` over `comptime types =
  reflect[T].field_types()` validates, but the instance reports "unknown
  type 'types'".
  - The elaborator resolves `f.T` over a bound handle (`resolve_reflected_type`,
    `comptime/eval.rs`) and nothing else in an annotation; `comptime FT =
    types[i]` then `var v: FT` is the working spelling.
  - Depends on 3.25 when the annotation sits in a `comptime for` body.
  - Model: Opus, Not Planned.

- [ ] **3.66 A handle chain in a call's type argument is read as a value**

  Problem: `_unqualified_type_name[reflect[T].field_at[i].T]()` reports
  "expected a type, found a value" at the instance.
  - The elaborator's `resolve_reflected_param_arg` rewrites a `ParamArg::Type`
    only; the chain arrives as a value argument.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.67 A clone that is still checked re-ranks an overloaded call**

  Problem: the pinned Mojo binds a call inside a generic body once, while it
  checks the body, and Mojito ranks the overload set again for every
  instantiation that takes the clone check.
  - An instance derived from its checked template inherits the template's
    choice (`assets/ok/template_overload_binding.mojo`,
    `overload-bound-in-generic-body`), with a scalar local or a branch
    (`assets/ok/template_overload_binding_local.mojo`), with a local of
    the parameter type handed to the call
    (`assets/ok/template_def_value_local.mojo`), from a struct method
    (`assets/ok/template_overload_binding_method.mojo`), and for a generic
    struct's static on a spelled receiver, even at an instance that
    collapses its family
    (`assets/ok/template_method_collapsed_static_overload.mojo`).
  - What remains is any body still outside the derivation classes for
    another reason: its instances are checked again, and rank the set
    again. It closes as section 1 widens the classes.
  - Depends on section 1: it closes as the derivation classes widen, and
    nothing in this section moves it.
  - Model: Fable, Not Planned.

- [ ] **3.68 Ordering a tuple of mixed element types is rejected**

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

- [ ] **3.69 A struct keyed on a `DType` named like a bundled binder is
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

- [ ] **3.70 A parametric nested `def` named as a value reports its marker**

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
  - Depends on 3.58, which decides which clone a bare reference to a
    parametric `def` names; the message follows from that.
  - Model: Opus, Not Planned.

- [ ] **3.71 A generic struct's `DType`-keyed method is never cloned
  without a discovery round**

  Problem: `holder.double[DType.int64](9)`, over `def double[dt:
  DType](self, a: Scalar[dt]) -> Scalar[dt]` on `struct Holder[T: Copyable
  & Movable & Deinitable]`, reports "invalid checked program: ... argument 0
  of 'Holder.double$y3:Int' has type Int64, declared Scalar[dt]"; the pin
  prints `18`.
  - The call keeps the per-instance method, whose `dt` lowers untyped,
    instead of retargeting to a per-call clone.
  - It runs once another inferred keyed call in the program (a free
    `twice(Int16(5))`) forces a discovery round, which is why
    `assets/ok/dtype_keyed_method.mojo`'s `Holder` case passes.
  - The same method on a plain struct clones without discovery.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.72 A method forwarding its own `DType` parameter to a sibling
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

- [ ] **3.73 `String` always owns a heap buffer, where upstream's has three
  representations**

  Problem: `String(literal)` allocates and copies the literal's bytes, while
  upstream points at the static bytes and copies only on the first mutation.
  - Upstream packs a static-constant, an inline (up to 23 bytes), and a
    reference-counted heap form into the same 24 bytes, flagged in
    `_capacity_or_data`; Mojito's `{data, size, cap}` has only the heap form.
  - No program output differs; allocation counts and `capacity()` do.
  - Port the static form first: every mutator, `__del__`, copy, and move
    must respect a non-owning flag, on both backends.
  - Depends on nothing. It is the representation gap the section intro keeps
    last.
  - Model: Opus, Planned.

- [ ] **3.74 A loan carried only by a container's element type does not keep
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

- [ ] **3.75 A copy of a loan-carrying value is taken as a borrow of its
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
  - The rule is the `BorrowViewResult` condition in `mc_infer.rs`: any
    loan-carrying struct result of a non-consuming method.
  - Dropping the borrow when the declared return names only the struct's own
    origin binders was tried. It lost the only link from `c.get()`'s result
    to the loans `c` carries, which 3.74 has to supply first.
  - Not yet checked against the pin, which is expected to accept both
    spellings.
  - Depends on 3.74.
  - Model: Fable, Planned.

- [ ] **3.76 A subscript of an element read through a `Span` of `Span`s fails
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

- [ ] **3.77 A compile-time-keyed `def` applied to a loan-carrying type is
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

- [ ] **3.78 Same-arity overloads of a generic method on a generic struct stop
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

- [ ] **3.79 A struct's callable-value parameter is undefined inside its
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

- [ ] **3.80 A loan-carrying element read out of a list does not keep its
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

- [ ] **3.81 A pointer field of a struct read out of a list cannot be
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

- [ ] **3.82 Mojito-specific shortcuts to move toward Mojo's shape** *(standing,
  any order)*

  Problem: parts of Mojito's stdlib lean on the Rust runtime where upstream
  is pure Mojo. Each is a candidate port, preferred over any new bridge
  (2026-09-07 direction). Depends on nothing, and neither port depends on the
  other. A bullet that waits on a numbered entry names it.

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

- [ ] **3.83 Behavioral divergences from the pinned Mojo — burn to zero**
  *(standing)*

  Every new divergence lands here with a probe or a `cases.tsv`
  `mojito-only` / `output-diff` row, and leaves when its probe promotes to
  an `assets/ok` fixture. Depends on nothing as a ledger. A bullet that waits
  on a numbered entry names it.

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
  - `where-on-trait-method`: a trait method declared with a trailing `where`
    clause (`def push[H: Movable](mut self, value: H) where conforms_to(H,
    Copyable): ...`) is accepted by Mojito and rejected upstream ("'where'
    clauses on trait methods are not supported", 2026-09-26).
    - A struct's witness of such a requirement is rejected as not matching
      the signature, with or without the same clause.
    - Pinned by `conformance/probes/where_on_trait_method.mojo`.
    - The lever is the trait-method arm of `check_trait`
      (`checker/traits.rs`), which compiles the clause onto the method's
      last binder.
    - Model: Opus, Not Planned.
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
    - Depends on 3.35, the same stand-in place through a sibling call's
      `mut` argument.
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
    - Depends on 3.40, which wants the same fallback to materialize a literal
      for a `ref` parameter: the two must agree on what it may materialize.
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
    - Depends on 3.7, the same unpack plan on an unnamed tuple.
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
    - Depends on 3.12, which tightens the returned-origin comparison this
      constructor path escapes.
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
    - The template-fact derivation materializes a folded name's `i / 2` to
      the template's `Float64` (`folded_arithmetic`); once the template
      records `Int` there it refuses, but the elaborator's literal fold then
      divides an `IntLiteral` pair into a float where the pin keeps `Int`.
    - Model: Fable, Planned.
  - `simd-infix-comparison`: Mojito's `<`/`<=`/`>`/`>=` are elementwise at
    every width and its `==`/`!=` compare lane by lane. The pin constrains
    the strict inequalities to `Scalar` and gives `==`/`!=` whole-vector
    meaning, pointing at `SIMD.lt(...)`, which Mojito now has (2026-09-13)
    along with `le`/`gt`/`ge`/`eq`/`ne`. Six corpus fixtures were respelled
    onto the methods.
    - Withdrawing the infix spelling is a leniency to remove, with fallout
      across the stdlib's SIMD bodies.
    - Until then only a width-one SIMD conforms to `Equatable` and
      `Comparable`; the pin's vectors conform too, with `==` reducing the
      lanes.
    - Depends on section 4's scalar comparison methods: once the infix
      spelling goes, `x.ne(y)` is the only ordered comparison left on a
      scalar, and it does not resolve there yet.
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
  - `two-live-accessor-interior-references`: `print(t.value_at(0),
    t.value_at(1))`, two `ref self` accessors returning an element's interior
    on one `var` owner, runs in Mojito and is rejected upstream ("use of
    invalidated interior reference 't.entries["element"]'"). Pinned by
    `conformance/probes/interior_reference_two_live_accessor_results.mojo`.
    - The pin takes the second call as a mutable borrow that invalidates the
      first result; a `List` subscript pair (`l[0], l[1]`) passes both.
    - Mojito records no conflict between the two results, so the lever is
      where a `ref self` call's interior reference meets a later mutable
      borrow of the same owner.
    - Found while deriving element-field reference results; not root-caused.
    - Model: Opus, Not Planned.
  - `wildcard-vector-parameter-on-a-def`: `def bits(value: SIMD[_, _])`
    runs upstream, while Mojito rejects the parameter with "not a valid SIMD
    element type: a non-DType argument". Pinned by
    `conformance/probes/simd_wildcard_to_bits_default.mojo`.
    - The `SIMD[_, _]` desugar (`synth.rs:desugar_simd_keyed_methods`) runs
      over struct methods only, where the `Hasher` protocol needs it; a free
      `def` keeps the wildcard spelling and resolves it as an annotation.
    - Found while probing the defaulted `to_bits()`.
    - Model: Opus, Not Planned.
  - `pointer-write-aliasing-embedded-origin`: `p.unsafe_write(Pointer(to=x))`
    over `p = unsafe_alloc[Pointer[Int, origin_of(x)]](1)` runs in Mojito and
    is rejected upstream ("aliasing values passed mutably to 'self' argument
    and passed mutably to 'value' argument"). Pinned by
    `conformance/probes/pointer_write_aliasing_embedded_origin.mojo`.
    - The pin counts the origin the receiver's element type embeds as a
      mutable access through `self`, which the argument's own mutable
      `origin_of(x)` aliases.
    - Mojito's exclusivity check reads the receiver's own provenance only.
    - Found while fixing a `Pointer` type argument's clone identity.
    - Model: Opus, Not Planned.
  - `pointer-type-argument-uninitialized-interior`:
    `unsafe_alloc[Pointer[Int, origin_of(a)._get_owned_interior["element"]]](1)`
    over a local `Array` runs in Mojito and is rejected upstream ("use of a
    never-initialized interior reference 'a["element"]'"). Pinned by
    `conformance/probes/pointer_type_argument_uninitialized_interior.mojo`.
    - The pin treats the spelled interior origin as a use of the local's
      interior, which nothing has initialized.
    - Mojito resolves the spelling to a place origin and checks no
      initialization of the interior.
    - Found while baking an interior-projected `Pointer` type argument into
      its clone (2026-09-28).
    - Model: Opus, Not Planned.

  Five divergences are retained on purpose and re-probed rather than fixed;
  they are listed in [`docs/non-goals.md`](non-goals.md).

- [ ] **3.84 An `Int` condition is rejected**

  Problem: `if x:` over `x: Int` runs at the pin, and Mojito rejects it with
  "type mismatch for if condition: expected Bool, found Int".
  - A width-one bool lane is accepted through the `Bool(x)` truthiness
    conversion; an `Int` never reaches that rule.
  - Found while deriving a runtime `if` in a keyed `def` (2026-09-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.85 A pointer returned by a call reads its owner after the owner is
  destroyed**

  Problem: `print(idp(p)[])` over `def idp[o: MutOrigin](v: Pointer[Int, o])
  -> Pointer[Int, o]` and `p = Pointer(to=x)` prints `None` when `x` has no
  later use; the pin prints `7`.
  - Drop elaboration destroys `x` before the dereference, since the call
    result's `ref.read` carries no loan of `x`.
  - Binding the result first (`var q = idp(p)`, then `q[]`) runs: the
    binding establishes the loan its type names.
  - A generic `id[T](v: T) -> T` over the pointer reaches the same path.
  - Found while fixing a `Pointer` type argument's clone identity
    (2026-09-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.86 A pointer held in a generic struct's parameter-typed field cannot
  be dereferenced in place**

  Problem: `print(h.item[])` or `print(h.get()[])` over `h =
  Holder(Pointer(to=x))`, whose `item: Self.T` holds the pointer, stops with
  "vm: ReadRef … received Int(7): expected reference handle"; the pin prints
  `7`.
  - Binding it first (`var p = h.get()`, then `p[]`) runs.
  - Storing such a pointer through `unsafe_write` and reading it back with
    `p.unsafe_offset(0)[][]` stops the same way.
  - A `_subtree` pointer (the extension) returned through a generic `def`
    over the holder (`var r = first(h)`, then `r[]`) stops the same way even
    bound first; a plain or interior-projected pointer runs there.
  - Found while fixing a `Pointer` type argument's clone identity
    (2026-09-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.87 A scalar does not splat implicitly into a multi-lane vector**

  Problem: `var v: SIMD[DType.int32, 4] = Int32(9)` is rejected with a type
  mismatch; the pin splats it through the implicit
  `SIMD.__init__(Scalar[dtype])`.
  - The same holds for a `Float64` into a `float64` vector and an `Int` into
    a `DType.int` vector. A `Bool` never splats implicitly upstream.
  - An exact literal already splats (2026-09-27): the checker records it as a
    literal materialization, which MIR lowers to a one-element `MakeSimd`.
  - A runtime scalar needs its own checked adjustment, since MIR
    materializes only a register whose checked type is a literal.
  - Spell `SIMD[DType.int32, 4](x)` until then.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.88 An annotated `comptime` literal ignores its declared type**

  Problem: `comptime ONE: Int32 = 1` binds `ONE` as an `Int`, so
  `ONE.dtype` prints `int` where the pin prints `int32`.
  - `comptime ONES: SIMD[DType.int32, 4] = 1` likewise prints `1` where the
    pin prints `[1, 1, 1, 1]`.
  - The checker binds every constant it can evaluate as `IntLiteral`
    (`checker/statements.rs`, `StmtKind::Comptime`) and never consults the
    annotation.
  - Found while probing literal splats (2026-09-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.89 A chained comparison copies its operands and accepts a middle
  operand the pin cannot copy**

  Problem: `b == c < d` over a `Copyable` struct prints `copy` three times
  and runs, where the pin rejects it: the middle operand `c` "cannot be
  implicitly copied".
  - The pin copies only the middle operand, which both links read, and
    demands `ImplicitlyCopyable` of it; over an `ImplicitlyCopyable` struct
    it prints no `copy` at all.
  - A two-operand comparison borrows a named operand its dunder reads
    (`borrow_nominal_place_argument` at `infer_infix`); the chain checks
    each link with no span (`ExprKind::Compare` in `checker/inference.rs`),
    so it records neither the borrow nor the middle operand's copy demand,
    and `compare_chain` lowers every operand as a copy.
  - Found while closing the operator-operand copies (2026-09-27).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.90 A generic nested `def` that captures does not compile
  natively**

  Problem: a generic nested `def` with any capture, such as
  `def inner[k: Int]() {x}` or one reading its enclosing function's value
  parameter, runs on the VM but is refused natively: "generic retained
  callable `outer$inner` has captures".
  - Native monomorphization specializes a generic nested `def` at each call
    by rewriting the indirect call into a direct call to the instance, which
    erases the environment (`native/mono/specializer.rs`, `CallIndirect`).
  - The lifted body takes its captures as leading reference parameters, so
    the rewrite must pass them and `infer_call` must skip them.
  - `conformance/probes/nested_def_reads_enclosing_parameter.mojo` pins it.
  - Found while letting nested `def`s read an enclosing value parameter
    (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.91 A nested `def` reading only an enclosing value parameter is
  not `thin`**

  Problem: `apply(inner)` against `def apply(f: def() thin -> Int)`, where
  `inner` reads only `outer[n]`'s `n`, is rejected ("expected def() thin ->
  Int, found def() capturing[...] -> Int"); the pin prints the result.
  - The pin reads a parameter as a compile-time value, so `inner` captures
    nothing.
  - Mojito gives a value parameter a runtime slot, and a nested `def` or
    lambda reads it through an implicit `imm` capture
    (`implicit_value_parameter_capture` in `checker/scopes.rs`).
  - Making it thin needs the parameter's value inside the lifted body:
    folded per instance, or passed as the lifted body's own parameter
    argument.
  - Found while letting nested `def`s read an enclosing value parameter
    (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.92 Variadic and tuple-literal elements carrying one mutable
  origin do not conflict**

  Problem: Mojito compiles `show(Span(xs), Span(xs))` over `def show[*Ts:
  Copyable](*args: *Ts)` and the literal `(Span(xs), Span(xs))` where `xs` is
  a `var`; the pin rejects both with "aliasing values passed mutably to
  'args' argument and passed mutably to 'args' argument".
  - The argument exclusivity rule judges the regular parameter slots and the
    receiver only (`check_argument_origin_exclusivity` in
    `checker/origins/exclusivity.rs`).
  - The pin judges each element collected by `*args` or a pack as an
    argument of its own, and a tuple literal as its initializer's call.
  - `conformance/probes/variadic_elements_share_mutable_origin.mojo` pins it.
  - Found while counting type-argument origins in argument exclusivity
    (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.93 A store through a dereferenced pointer to a loan-carrying
  pointee fails at run time**

  Problem: `q[] = Span(xs)` where `q` is an
  `unsafe_alloc[Span[Int, origin_of(xs)]](1)` fails on the VM with "use
  after Pointer deallocation", and beside other allocations fails MIR
  verification with "place rooted at slot 5 lacks complete checked type
  metadata"; the pin prints the element.
  - It is the one spelling the pin accepts for writing a span over a mutable
    list through such a pointer, since `q.unsafe_write(Span(xs))` aliases.
  - `conformance/probes/pointer_deref_store_loan_carrying_pointee.mojo` pins
    it.
  - Found while counting type-argument origins in argument exclusivity
    (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.94 A homogeneous `*args` over a loan-carrying element type is
  rejected**

  Problem: `two(Span(xs), Span(xs))` over `def two[T: Copyable](*args: T)`
  with `xs` a read parameter is rejected ("type mismatch for variadic
  argument to 'two$y9:Span[Int]': expected Span[Int], found Span[Int]"); the
  pin runs it.
  - The cause is not diagnosed: the two types print alike, so they differ
    in an origin slot the message does not show.
  - `conformance/probes/homogeneous_variadic_loan_carrying_element.mojo`
    pins it.
  - Found while counting type-argument origins in argument exclusivity
    (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.95 A write through a reference skips the overwritten value's
  `Deinitable` check**

  Problem: in a `def` over `T: Copyable`, `ref r = x; r = v.copy()` and
  `same(x) = v.copy()` (a call returning `ref[origin_of(x)] T`) run on
  Mojito, where the pin rejects both: the overwritten `T` is "abandoned
  without being explicitly destroyed".
  - Mojito accepts a program the pin rejects, so this is a divergence.
  - The plain store `x = v.copy()` is rejected alike on both.
  - The abstract destruction walk (`explicit_destroy.rs`) checks a store to
    a named variable, not a `SetPlace` whose place is a reference binding
    or a reference-returning call.
  - `conformance/probes/reference_write_non_deinitable.mojo` pins it.
  - Found while making a reference-returning call an assignment target
    (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.96 An in-place operator cannot apply to a module `def`'s
  reference result**

  Problem: `text(s) += "d"` for `def text(ref t: String) ->
  ref[origin_of(t)] String` is rejected ("an in-place operator on the
  reference returned by 'text()'"), where the pin prints `ad`.
  - A method's reference result takes the operator
    (`n.name_ref() += "b"`), as does a `ref` binding of `text(s)`, and a
    scalar referent takes `+=` through the builtin operator.
  - A module `def`'s reference result is carried only by the
    `ReferenceResult` adjustment at the call's span, which the operator's
    `AugmentedInPlace` record at the same span replaces; a method's is also
    in its selected-call contract.
  - Carrying it in a selected-call contract for module `def` calls, as the
    MIR's `reference_result` already prefers, would lift it.
  - `conformance/probes/reference_call_inplace_operator.mojo` pins it.
  - Found while making a reference-returning call an assignment target
    (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **3.97 An overloaded method's symbol is spelled one way where it is
  declared and another where it is called**

  Problem: for an overloaded method with a callable-typed parameter or a
  bounded binder of its own, the checker's call target names a symbol MIR
  never declares.
  - The checker keys `pick[T: Copyable](self, a: T, b: Int)` as
    `First.pick$ov$T$Copyable$Int`; MIR declares `First.pick$ov$T$Int`,
    since `lowered_method_name` sees only the struct's binders.
  - A callable-typed parameter keys as `F$Callable$def…` at the call and
    `F$$u3C$function$u20$type$u3E$` at the declaration (`Variant.set`).
  - Only per-call clones reach these methods today; a call left on the
    template path stops with "vm: unknown method".
  - The per-call clone filter matches the call side's spelling against
    `MethodBinderOwners::call_qualifier`, and a callable-typed overload
    falls back to minting every same-named member for the request.
  - Found while keying per-call clones by overload (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.98 A generic `def` reads a module constant where a local shadows
  it**

  Problem: `var TWO = 100; return TWO` in `def run[T: Copyable](s: T)`,
  beside `comptime TWO = 2`, returns `2`, where the pin returns `100`.
  - A non-generic `def` returns `100`; only the specialized clone's body
    reads the module constant.
  - The clone's materialization substitutes module constants by name
    (`materialize_block`) without honoring a local binding that shadows
    one.
  - Found while folding trait requirement defaults (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.99 A trait requirement's default reading more than module
  constants is rejected**

  Problem: `def scale[n: Int](self, value: Int, factor: Int = n)` in a
  trait is rejected as unsupported, where the pin accepts it.
  - A requirement default is folded to a literal at the trait
    (`requirement_default` in `checker/traits.rs`), because a call through
    a bound spells it at the call (`checker/bound_defaults.rs`), where a
    name may mean something else.
  - A literal, a module constant bound to one, and a compile-time `Int`
    expression over module constants fold; a method's own parameter, a
    call, or a constant of another shape does not.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.100 A default naming a binder in scope fails at run time**

  Problem: `V[3]().m()`, beside `def m(self, x: Int = Self.n * 2)` in
  `struct V[n: Int]`, checks but stops on the VM with "non-constant default
  for parameter 'x'", where the pin prints `6`.
  - MIR lowers a default that is no literal as a zero-parameter function
    (`lower_default` in `mir.rs`), which has no binder of its own, so a
    default spelling `Self`, a struct or enclosing function's parameter, or
    the declaration's own stays unlowered and the VM refuses its slot.
  - A nested `def`'s `x: Int = n` inside `def outer[n: Int]()` fails the
    same way.
  - `def g[n: Int](x: Int = n)` is rejected earlier, with "Undefined
    variable 'n'", where the pin prints the argument.
  - The fix is to substitute the instance's binders into the default, or
    to give the default function the declaration's parameters.
  - Found while running constructed defaults (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.101 A list literal default for a `List` parameter is an `Array`**

  Problem: `def grow(var xs: List[Int] = [1, 2])` is rejected with "type
  mismatch for default value of 'xs': expected List[Int], found Array[Int,
  2]", where the pin accepts it and a call leaving `xs` out gets a fresh
  list.
  - A default is inferred without the parameter's type as context, so an
    uncontextualized display types as `Array`.
  - Found while running constructed defaults (2026-09-28).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **3.102 A static method cannot read its struct's value parameter**

  Problem: `return Self.k` in a `@staticmethod` of `struct W[k: Int]` fails
  at run time with "field access on non-struct None", where the pin prints
  `5` for `W[5].st()`.
  - The erased static body reads `Self.k` as a field of a `self` it does
    not have (`receiver_value_parameter_read` in `mojito-mir`).
  - A call there inferring a value parameter from `Counter[Self.k]` fails
    MIR verification with "required compile-time value parameter 'n' is
    missing" for the same reason.
  - `conformance/probes/static_method_reads_struct_value_parameter.mojo`
    pins both.
  - Found while inferring value parameters from argument types
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### 4. Grow The CPU Standard Library *(demand-first)*

- [ ] **4.1 Collection API parity**

  Goal: grow the tuple, slice, optional/variant, and String surfaces toward
  the audited head (`docs/features.md` records what lands). The tasks below
  are in impact order: soundness of the executable oracle first, then
  everyday spellings that reject today, then parity details. No task
  depends on a later one. Every bullet is a conscious, recorded limit, a
  task closes when its bullets are done, and a residue found inside a task
  moves to the task that owns its fix.

  Depends on nothing as a whole, but one residue below is owned elsewhere:
  the `var` collision across unrolled iterations is section 3's `comptime
  for` scoping entry.

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
       element type).
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
       prelude qualification the other annotations get.
     - A list display as an argument to an explicitly applied constructor
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
  (`docs/features.md`). Depends on nothing; the two destruction-order
  divergences the stage found are section 3's.

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
  - Depends on nothing. Section 3's `simd-infix-comparison` divergence waits
    on it: withdrawing the infix spelling leaves `x.ne(y)` as the only
    ordered comparison on a scalar.

- [ ] **4.4 Time, random, and testing slices**

  Goal: deterministic testable cores, with host-dependent behavior behind
  runtime services. Depends on nothing.

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
  - Depends on nothing.

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
  - Depends on nothing.

- [ ] **5.3 Compile-time performance**

  Problem: Hello World is 1.5 s release / 7.9 s debug on the reference
  machine (`docs/performance.md`), and the first checker pass over the
  prelude is now most of it.
  - A body whose inputs are unchanged since the previous checker pass takes
    that pass's facts (`checker/body_carry.rs`), so the later passes of a
    compilation are cheap. The first pass still infers every prelude body.
  - `checked_var_types` scans the whole expression table per variable, and
    `explicit_destroy` re-derives deinitability per struct per pass.
  - Only then cache the elaborated/checked stdlib across processes.
  - Depends on nothing. 5.4, 5.5, and 5.6 are named costs under the same
    goal and land in any order.

- [ ] **5.4 A carried checker pass still copies every fact it keeps**

  Problem: a pass that carries nearly every body still costs about a third
  of an inferring pass (`check_program.bodies` under `--timings`).
  - The carry clones each logged entry into the fresh checker's stores and
    hashes every site's syntax, once per pass (`body_carry.rs:carry_body`,
    `def_syntax_hash`).
  - Moving the previous pass's stores into the fresh checker and removing
    the entries of the bodies it infers would replace the copies.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **5.5 A clone appended before older clones loses its carry record**

  Problem: a discovery round that inserts a new clone ahead of existing
  ones in the elaborated tree renumbers the older clones' duplicate syntax
  identities, so their records no longer match and they are inferred again.
  - `--timings` notes show them as `body_facts.carry_refused … its syntax
    changed` and `no record in the previous pass` (about a hundred per
    round for Hello World).
  - The final re-key (`ast.rs:rekey_syntax`) numbers replacement identities
    in traversal order. Keying a clone's occurrences by the template
    identity and copy index, as the template mechanism does, would keep
    them stable.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **5.6 Request identity names binding identities through origins**

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

- [ ] **5.7 Feature and target options**

  Goal: checked CLI/build configuration recorded in artifacts and
  diagnostics. Depends on nothing.

- [ ] **5.8 Compiled package artifacts**

  Goal: a versioned `.mojoc` representation (modules stay non-first-class).
  Depends on nothing. Per-directory resolution order:
  1. source package
  2. `.mojoc`
  3. source module
  4. legacy `.mojopkg`

- [ ] **5.9 Debugging metadata and inspection**

  Goal: stack/source diagnostics, MIR inspection, and debugger-oriented
  value rendering. Depends on nothing.

- [ ] **5.10 Testing tools**

  Goal: Mojito-native assertions, expected-error tests, and
  differential-harness integration. Depends on nothing.

- [ ] **5.11 Distribution reproducibility gate** *(last)*

  Goal: the release check rebuilds, tests, documents, and reproduces
  conformance from the crates.io archive alone. Depends on every other entry
  in this section, and stays last whatever else moves.

### 6. Code Organization Follow-Ups *(behavior-preserving)*

The 2026-09 module split (`docs/symbol-map.md`) removed every file over
3,000 lines. What remains needs semantic extraction, not line moves.

- [ ] **6.1 Split `expr_unconverted`**

  `mir/lower_expr/expr.rs` (about 2,090 lines) is one match over
  `ExprKind`.
  - Extract arm groups into `Flatten` methods.
  - Depends on nothing.

- [ ] **6.2 Split `infer_method_call`**

  `checker/method_calls/mc_infer.rs` (about 1,530 lines) is one method.
  - Extract receiver-family branches beside `selection`, `statics`, and
    `builtin_types`.
  - Depends on nothing. It moves code several section 3 entries name as
    their lever, so it lands between them rather than beside them.

- [ ] **6.3 Split `verify_instruction`**

  `mir/verify/instr.rs` (about 1,320 lines) is one match over `MirInstr`.
  - Extract per-family check helpers.
  - Depends on nothing.

- [ ] **6.4 Shrink the 2 kloc band**

  Split these further only along a cohesive seam, while touching them:
  - `checker/traits.rs` (2,629), `mir/lower_stmt.rs` (2,595),
    `checker/inference.rs` (2,520), `checker/statements.rs` (2,471),
    `ast.rs` (2,464), `mir.rs` (2,426), `checker/type_resolution.rs`
    (2,395), `runtime.rs` (2,235), `checker.rs` (2,232),
    `comptime/rewrite.rs` (2,179), `checker/declarations.rs` (2,118),
    `mir/text/write.rs` (2,116), `backend/vm/exec.rs` (2,085).
  - Depends on nothing. Split one of these only while touching it for
    another task.

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
  how much the entry moves that section's goal. Renumber when one lands.
- Every checkbox carries a **Depends on** bullet, immediately before its
  **Model:** bullet: the entries it needs, by number, or `Depends on
  nothing.` Count an entry that merely makes another one easier as a
  dependency, and name a dependency in another section by that section rather
  than guess its number. An entry with no **Model:** bullet says it in its
  goal paragraph instead, and a standing ledger says it in its intro and
  again on any bullet that waits on a numbered entry.
- A dependency is never below the entry that needs it. Where impact and
  dependency disagree, the dependency wins, and the cluster sits where its
  most important member would.
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
