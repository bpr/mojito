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

Every entry carries a stable ID (`R12`). An ID is issued once, by
`scripts/roadmap.py new-id`, and is never renumbered or reused, so cite an
entry anywhere — another entry, a probe, a plan, a commit — by its ID. Work
is grouped into tracks, one per heading below, each named by its `Track:`
slug. An entry's position in its track is its priority, and nothing else.

Each entry's **Depends on** bullet names the IDs it needs, or `nothing`. An
entry that merely makes another one easier counts as a dependency, and an ID
no longer on the roadmap counts as done. The work order is computed, never
maintained by hand: `scripts/roadmap.py list [--track T]` walks the open
entries top-down and puts each one's open prerequisites, from any track,
immediately before it, and `scripts/claude_loop.py` follows that order. So
landing, filing, or moving an entry edits that entry alone. The **Model:**
bullet is an estimate, not a sort key. An entry marked *(standing)* is
guidance kept in view, never scheduled.

Next free ID: **R532**.

## Ordered Work

### Mojo's Pipeline Order: Parametric MIR And One Elaborator

Track: `pmir`.

Mojo checks a parametric body once, keeps it as parametric IR, checks
lifetimes on that IR, and instantiates everything in one elaborator. Mojito
checks every body once, with its parameters symbolic, and `native::mono`
instantiates every generic body below the MIR waist for both backends; the
VM runs concrete MIR only. The staged plan that closed the gap is
[`docs/parametric-mir-plan.md`](parametric-mir-plan.md); the stage each entry
belongs to is named in its title.

Scope: what remains of the plan's stages, and what the pipeline order still
leaves to close. A defect found on the way is filed by its kind; a
divergence from the pin goes to a catch-up track, however small.

- [ ] **R516 An applied module dictionary or set display constant is
  spelled at each read, not bound as a parameter expression**

  Problem: `comptime D = {"a": f(1), "b": 3}` has no parameter-expression
  form, so the elaborator spells the display where a body reads it.
  - A method call or subscript of the constant, `materialize[D]()`, a
    `comptime for` over it, and another constant's initializer each get the
    display in place (`Elab::spell_applied_displays`), and a runtime value
    read becomes the request `comptime(<display>)`
    (`comptime/requests.rs`).
  - Decision D3 (`docs/notes/ctfe-request-path.md`) wants the constant
    bound as `Apply(thunk)` like a list display is
    (`Checker::module_lifted_application`). A module-scope
    `comptime C = M.copy()` remains this task's binding-shape gap;
    the body-local copy now freezes and materializes.
  - Fixture: `assets/ok/ctfe_module_display_constant_requests.mojo`.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R517 A closed module constant that is not a scalar is still spelled
  into bodies as a literal**

  Problem: a module struct value, tuple, vector, `DType`, or closed display
  constant is substituted into every body by the elaborator
  (`materialize_block`), while a scalar or applied constant is a checked
  `ParamValue` fact lowered as `Const::Param`.
  - The checker's declaration pass classifies only folded `Int`s, literals,
    and applications (`Checker::module_constant_value`); the rest keep the
    elaborator's substitution, so their reads carry no parameter-value fact.
  - A list-valued application read whole in a lifted thunk has no
    register construction either (`mono::substitute::value_parameter_constant`
    returns none for a `CtValue::List`), which is why a module list display
    is read element by element (`ParamKind::ListGet`) or spelled.
  - Depends on R516.
  - Model: Fable, Not Planned.

- [ ] **R11 (P6) The standard library is checked again in every
  compilation**

  Problem: Mojo imports a package without checking its source again, and
  Mojito re-parses and re-checks the bundled library each time.
  - Serialize verified, drop-elaborated generators as a package artifact.
    That boundary is Mojito's choice. Upstream's `.mojoc` holds the
    post-parse IR, written before lowering.
  - A package keeps every exported template, reached by an entry or not.
  - Importing skips the source check. It still validates the artifact and
    checks each instance's obligations.
  - The bundled library is the first consumer, built once per compiler build.
  - Depends on R10.
  - Model: Fable, Planned.

- [ ] **R474 (P5) A closed parameter argument outside a generic call's
  brackets still lowers as run-time code**

  Problem: the caller still emits a `const` register for `Counter[4](1)`'s
  `4`, for the inferred `n = 4` of `size(Counter[4](1))`, and for a string
  argument such as `f["x"]()` or `external_call`'s callee, where Mojo's call
  carries each as a parameter attribute and emits nothing.
  - A generic call's closed bracket argument is compile-time data on the
    call (`SemanticAdjustment::FoldedParameterArguments`); these shapes have
    no such record.
  - A struct instantiation carries no `InstantiatedArguments`, the inferred
    values lower through `Flatten::inferred_param_arg_regs`, and a string is
    excluded because `mojito-pliron/src/lower/externs.rs` reads
    `external_call`'s callee off its register.
  - The registers are dead after `native::mono`, so no output differs; a
    struct's tuple value parameter would leak as R468 did, but R463 rejects
    reading one.
  - Fix: record the folded spans on the struct instantiation, read inferred
    values from the call's instantiated arguments, and move the
    `external_call` read to them.
  - Found while landing R468 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R347 (P3d) A static call on a `Tuple` type over a pack still open
  finds no method**

  Problem: `return Tuple[*Ts].__len__()` in `def plain_len[*Ts:
  Movable](t: Tuple[*Ts])` stops with "type 'Tuple[…]' has no method
  '__len__'", where the pin answers the length.
  - `Tuple[*Ts.reverse()].__len__()` and
    `Tuple[*TypeList._concat[Self.Ts.values, OtherTs.values]()].__len__()`
    stop the same way.
  - An instance member on such a tuple runs: `t^.reverse()`, `other[0]`.
  - A `Tuple` type over an open pack has no declaration to serve a static
    member until `Tuple` is a generator.
  - Depends on R4.
  - Model: Fable, Planned.

### Native Backend

Track: `native`.

The ABI-bump collector is last whatever else moves, because it batches every
change that needs a new `MJRT_ABI_VERSION`.

- [ ] **R433 Natively, a struct pack element's `write_to` takes the
  reflective default**

  Problem: `a[i].write_to(w)` over a pack element whose type declares its
  own `write_to` prints `P(1)` on the VM and at the pin, but `P(x=1)`
  natively.
  - Probe: `def f[*Ts: Writable](mut w: String, *a: *Ts)` writing each
    `a[i]` into `w`, called as `f(s, P(1), 2)`.
  - `"{}".format(P(1))` reaches the same call through the bundled
    `_FormatCurlyEntry._format`, so native `format` of such a struct is
    wrong too.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R434 Natively, a struct without its own `write_repr_to` cannot be
  formatted through a pack**

  Problem: `"{}".format(P(1))` over a struct declaring only `write_to`
  stops natively with "unsupported repr of `P` without compiled
  write_repr_to", where the VM and the pin print it.
  - `_FormatCurlyEntry._format` holds both `write_to` and `write_repr_to`
    calls per element, so every element type instantiates the repr branch.
  - The reflective default `write_repr_to` is synthesized in the checker
    (R424) and has no native body.
  - Found while landing R421 (2026-10-06).
  - Depends on R424.
  - Model: Opus, Not Planned.

- [ ] **R277 Native `Tuple.concat` double-frees a `String` element of its
  argument**

  Problem: `var c = u^.concat((String("x"), 3.5))` prints the result and
  then traps with "vm: double free of Pointer allocation" natively, where the
  pin and the VM run to completion.
  - `reverse`, and `concat` over a `String` in the receiver, run natively
    since the members move their elements out through pointers
    (2026-10-06).
  - An argument of `Int`s is clean, and so is one whose element is a struct
    with a destructor (`assets/ok/named_result_transferred.mojo`).
  - The member takes each element of `other` through a pointer and marks
    `other.storage` destroyed, so the suspect is the presence flag of a
    `deinit` parameter that is not the receiver.
  - Pinned by `conformance/probes/native_tuple_transform_string_element.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R469 Native `Tuple` construction copies a fresh struct element in
  and leaks the original**

  Problem: `var t = (1, Q(8, "x"))` runs `Q`'s copy initializer natively and
  never destroys the temporary, where the pin and the VM move it in and
  print `1 / del 8 / end`.
  - Native prints `copy 8 / 1 / del 8 / end`.
  - Every read of a value parameter holding such a struct constructs one
    (R466), so the copy shows once per read there.
  - Pinned by `conformance/probes/native_tuple_struct_element_copied.mojo`.
  - Found while landing R466 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R299 Native lowering cannot lay out a struct a served pack binds
  only as an element**

  Problem: `show(String("tmp"), [7, 8])` and `show(Named("k", w), …)` into
  a template-served `def show[*Ts: Writable](*args: *Ts)` stop natively with
  "unsupported aggregate layout (struct 'List' has no MIR declaration to lay
  out)", where the VM and the pin print
  (`assets/ok/pack_argument_destroyed_after_call.mojo`,
  `assets/ok/pack_element_temporaries.mojo`, both `exe-differential` rows
  of `conformance/pliron-parity.tsv`).
  - The struct appears only as an element of the call's pack, so no
    declaration of it reaches the native program's layout table.
  - Before R252 landed, `pack_element_temporaries` stopped earlier, at
    "unsupported heterogeneous runtime pack projection": a folded constant
    index into a served `def`'s pack was projected by stride.
  - Found while landing R252 (2026-10-04).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R262 Finding a nested instance's method clone still spells every
  level of its type**

  Problem: when the checker minted a per-instantiation clone of a
  template's method, the elaborator names the clone an instance needs by
  respelling the instance's whole argument type, so a deeply nested
  instance of that template pays its depth on each lookup.
  - `template_spelled_arguments` rebuilds each level under its template
    name, and `mangle` renders the whole type for the clone's symbol.
  - A template with no minted clone of the method skips both
    (`instance_method_clone`), so the nesting probes are linear.
  - The lever is a clone index keyed by the instance's arguments rather
    than by a spelled symbol.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R275 Native `repr` of a `SIMD` vector wider than one lane is
  unsupported**

  Problem: `repr(SIMD[DType.int32, 2](1, 2))` prints
  `SIMD[DType.int32, 2](1, 2)` at the pin and on the VM, but the native
  backend stops with "unsupported type `Simd { … width: Known(2) }`".
  - Pliron's `lower_repr_builtin` (`lower/methods.rs`) labels only scalar
    registers; a vector register reaches no arm.
  - The text is upstream's `SIMD.write_repr_to`: `SIMD[`, the dtype's repr,
    the width, then the lanes as `print_simd` writes them.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R18 Native runtime ABI bump: land every change that needs a new
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

- [ ] **R435 Natively, `Error` over a local `comptime` string is
  unsupported**

  Problem: `comptime msg = "boom"` then `raise Error(msg)` runs on the VM
  and at the pin, while the native backend reports "unsupported string value
  in register".
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Catch Up To Current Mojo: Ownership, Origins, And Destruction

Track: `ownership`.

Every catch-up track closes a gap between Mojito and the pinned Mojo. Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

- [ ] **R529 A `ref` element of a `Tuple` field loses its reference handle**

  Problem: `struct RefTuple[o]: var values: Tuple[ref[o] Int, ref[o] Int]` constructed from two refs prints wrong or fails with "expected reference handle" on read, since `Tuple` became a library struct generator.
  - Mojito-only: the pin has no `ref` element types; `vm_test::nested_reference_aggregate_preserves_handles` pins it.
  - Either carry handles through `Tuple`'s pack storage or withdraw the form as an extension.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R473 A discard `_ = x` copies its source instead of taking it**

  Problem: `var x = Q(1, "y")` then `_ = x` as `x`'s last use prints
  `del 1` twice in Mojito, where the pin destroys one `Q`.
  - The same holds for `var r = q; _ = r` over a value parameter `q`.
  - Mojito copies `x` into the discard and destroys both, where the pin
    moves the last use.
  - Found while landing R467 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R352 A pointer whose origin was cast reads the origin's place, not
  its pointee**

  Problem: `var p = Pointer(to=b).unsafe_origin_cast[origin_of(a)]()` then
  `print(p[])` prints `a`'s value in Mojito, where the pin prints `b`'s.
  - `pointer_deref_place` (`crates/mojito-mir/src/mir.rs`) substitutes the
    loan's place for the pointer's target.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R355 A view element moved by `Tuple.reverse` or `Tuple.concat`
  loses its loan on the storage it views**

  Problem: `var t = Tuple(StringSpan(s), 1)` then `var r = t.reverse()` and
  `print(r[0], r[1])` stops in the VM with "use after Pointer deallocation",
  where the pin prints `1 hello`.
  - The result carries no loan on `s`, so `s` is destroyed at its own last
    use, before the result is read.
  - The transforms write their named result through pointers, which records
    no origin for the caller to keep.
  - Before the transforms were declared in Mojo the same program was
    rejected with a type mismatch on the result.
  - Reading `t[0]` without a transform prints.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R496 A reference returned through a temporary `Span` is stale on
  the VM**

  Problem: `def first(ref l: List[Int]) -> ref[l] Int: return Span(l)[0]`
  stops on the VM with "stale reference to frame N", where the pin prints
  the element.
  - A VM reference handle is a frame slot plus a projection, so an element
    reached through the temporary span's pointer stays rooted at the slot
    the body drops before it returns.
  - A result bound to a `ref[ImmStaticOrigin]` register is already
    forwarded as the element's heap pointer
    (`VmBackend::static_reference_result`), which serves
    `ParameterList.__getitem__`.
  - The general fix is a reference handle that designates heap storage
    whenever its projection crosses a pointer.
  - Found while landing R325 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R20 A value live after a region is never destroyed when the region
  raises out of the function**

  Problem: `var b = Thing(); if cond: raise Error("boom"); look(b)` never
  runs `b`'s destructor when the raise propagates, where the pin destroys
  `b` on the raise edge.
  - The same leak follows a raising call with the value live after it, a
    raise inside a loop, a raise in an `else` arm, and a raise inside a
    `comptime if` arm; only a value dead at the raise is destroyed, since
    its drop precedes the raise.
  - A `try` in the same function cleans up its body's locals on the
    exceptional edge; a raise that leaves the function has no cleanup list.
  - Decide where the cleanup lives: a per-function unwind list the VM and
    the native runtime both run, or drops elaborated on the raise edge.
  - Probe: `conformance/probes/raise_path_live_value_leaks.mojo`
    (`docs/notes/comptime-region-ownership.md`, `c7`).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R30 A `var` of an opaque type is not required to be `Deinitable`**

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

- [ ] **R31 An indexed element that is implicitly copyable may be
  transferred**

  Problem: `return x[0]^` over a `List[P]` with an `ImplicitlyCopyable`
  struct `P` runs on Mojito and copies the element, where the pin reports
  "expression does not designate a value with an origin".
  - The pin admits the transfer only for a trivial register value such as
    `Int`, and warns that it has no effect.
  - Mojito's `Transfer` inference (`checker/inference.rs`) admits every
    implicitly copyable indexed element.
  - Narrowing the test to trivial register values is the lever.
  - Probe: `conformance/probes/indexed_copyable_element_transfer.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R32 Owned iteration of a variadic pack moves elements the pin copies**

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

- [ ] **R34 `@explicit_destroy` is accepted on a type that is `Deinitable`
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

- [ ] **R48 A `String` element copied out of a `Span` fails**

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

- [ ] **R49 A generic method moving its `var` parameter into a sibling call's
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
  - Divergence R167 (`moved-parameter-into-local-collection`) is the same
    stand-in place and closes with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R54 A literal passed to a `ref` parameter stops at run time**

  Problem: `look(3)` against `def look(ref other: Int)` is accepted and then
  fails with "reference binding to a non-place expression".
  - The pin materializes the literal and binds the parameter to the temporary.
  - Probe: `conformance/probes/literal_to_ref_parameter.mojo`.
  - It materializes through the same fallback divergence R169
    (`ref-binding-register-value`) wants narrowed, so the two answers must
    agree.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R406 A subscript of a call result handed to `print` stops at run
  time**

  Problem: `print(mk()[0])` over `def mk() -> Tuple[Int, Int]` (or `->
  List[Int]`) prints `1` at the pin and stops in Mojito with "reference
  binding to a non-place expression".
  - `var x = mk()[0]` and `print(mk()[0] + 1)` run, so only a subscript
    result forwarded by reference to `print` reaches the failure.
  - A tuple display subscripted as a temporary (`(1, 2)[0]`) runs: the
    display's `ConstructCollection` carries its materialized owner.
  - Found while landing R367 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R87 A loan carried only by a container's element type does not keep
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

- [ ] **R88 A copy of a loan-carrying value is taken as a borrow of its
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
  - The rule is the `BorrowViewResult` condition in `method_calls/call_contract.rs`: any
    loan-carrying struct result of a non-consuming method.
  - Dropping the borrow when the declared return names only the struct's own
    origin binders was tried. It lost the only link from `c.get()`'s result
    to the loans `c` carries, which R87 has to supply first.
  - Not yet checked against the pin, which is expected to accept both
    spellings.
  - Depends on R87.
  - Model: Fable, Planned.

- [ ] **R89 A subscript of an element read through a `Span` of `Span`s fails
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

- [ ] **R93 A loan-carrying element read out of a list does not keep its
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

- [ ] **R94 A pointer field of a struct read out of a list cannot be
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

- [ ] **R101 An initializer call is not judged by argument aliasing**

  Problem: `Tuple(Span(xs), Span(xs))` over a `var xs` runs and prints
  `3`; the pin rejects it with "aliasing values passed mutably to 'args'
  argument and passed mutably to 'args' argument in 'Tuple[...]'
  initializer call".
  - Neither the place rule (`check_call_aliasing`) nor the carried-origin
    rule (`check_argument_origin_exclusivity`) runs on a constructor call;
    both run on free calls, methods, and static methods.
  - A tuple literal is judged as its initializer's call already
    (`check_tuple_literal_exclusivity`).
  - `conformance/probes/initializer_arguments_share_mutable_origin.mojo`
    pins it.
  - Found while judging variadic elements in argument exclusivity
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R103 A tuple literal holding a pointer to a local is rejected**

  Problem: `var p = (Pointer(to=x), 2)` is rejected with "not a
  compile-time value: type pack contains a type which cannot be
  materialized in source"; the pin prints `p[1]`.
  - `(Span(xs), Span(xs))` over a read parameter materializes, so the
    pointer's origin spelling is the likely difference.
  - `conformance/probes/tuple_literal_pointer_to_local.mojo` pins it.
  - Found while judging variadic elements in argument exclusivity
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R104 A type mismatch between two loan-carrying types prints them
  alike**

  Problem: `two(Span(xs), Span(ys))` over `def two[T: Copyable](a: T, b:
  T)` or `(*args: T)` is rejected, as the pin rejects it, but with "type
  mismatch for type parameter 'T': expected Span[Int], found Span[Int]".
  - The pin names the origins: "cannot be converted from 'Span[Int,
    origin_of(ys)]' to 'Span[Int, origin_of(xs)]'".
  - `Ty`'s display drops a struct's origin arguments, so every mismatch
    that differs only in an origin reads as no mismatch at all.
  - Found while binding a homogeneous `*args` element's origins
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R111 An evaluated default handed to a borrowing parameter is
  never destroyed**

  Problem: `use()`, beside `def use(r: R = R(String("dflt")))` over a
  struct with a `__deinit__`, never runs that destructor on the VM or
  natively, where the pin prints `drop dflt` when the call returns.
  - An explicitly passed temporary (`use(R(String("x")))`) is destroyed
    on both sides, and so is the default of a `var` parameter, which the
    callee owns.
  - The VM's `bind_for_call` runs the default function into a register
    no drop elaboration sees. Native lowering matches it, and frees the
    value's buffers without running the destructor.
  - Probe: `conformance/probes/borrowed_default_argument_destructor.mojo`.
  - Found while evaluated defaults were lowered natively (2026-09-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R114 An implicit conversion through a consuming constructor copies a
  parameter-typed place**

  Problem: `self.keep(self.item)` with `keep(self, box: Wrapper[Self.T])` and
  `@implicit def __init__(out self, var value: Self.T)` runs for
  `T: Copyable`, copying a `List[Int]` implicitly, where the pin rejects the
  declaration.
  - The pin reports "value of type 'T' cannot be implicitly copied, it does
    not conform to 'ImplicitlyCopyable'".
  - Mojito's check of the method records no copy at the conversion's
    consuming parameter.
  - Probe: `conformance/probes/consuming_conversion_copies_parameter.mojo`.
  - Found while the template's MIR replaced per-instance method clones
    (2026-10-01).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R115 A second `Dict` lookup stales a view of an earlier value**

  Problem: `var v = d["a"].rstrip()` then `var u = d["b"].rstrip()` then a read
  of `v` is rejected with "use of invalidated interior reference 'v' to
  'd["value"]["bytes"]'", where the pin prints both.
  - A `Dict` lookup defines a fresh `value` generation, which replaces the
    earlier one (`record_replacing_interior_reference`).
  - The view lends `d["value"]["bytes"]` below the earlier generation, so the
    second lookup stales it.
  - The pin names the same interior, and a lookup does not replace it there.
  - A single view of a `Dict` value runs
    (`assets/ok/list_element_view_method_result.mojo`).
  - Probe: `conformance/probes/dict_value_view_second_lookup.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R116 A `^` transfer of a `ref` binding fails in MIR verification, not
  in the checker**

  Problem: `ref r = s` then `var t = r^` is rejected with "unsupported
  specialized MIR that does not verify: ... binding of ref String to a slot
  of type String", where the pin reports "expression does not designate a
  value with an origin".
  - A borrowed loop binder (`for x in xs: var y = x^`) fails the same way.
  - The program is rejected on both sides, so only the diagnostic and the
    phase that owns it are wrong.
  - `check_transfer_source` (`checker/places.rs`) skips a binding of
    reference type, and is where the rejection belongs.
  - Whether a `ref` parameter over a mutable origin may be transferred is
    still to be observed on the pin.
  - Probe: `conformance/probes/reference_binding_transfer.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R117 Converting a `List` to a `Span` copies its elements**

  Problem: `var s: Span[Dup, origin_of(xs)] = xs` runs each element's copy
  constructor twice, where the pin builds the view and copies nothing. The
  copies are never destroyed.
  - An element read through the view (`print(s[0])`, `g(s[0])`) copies once
    more, where a `List`, `Tuple` or pack element is read in place.
  - The view read keeps the copy because lending it takes the path R48
    reports as failing.
  - The conversion's copies are not root-caused.
  - Probe: `conformance/probes/span_conversion_copies_elements.mojo`.
  - Depends on R48.
  - Model: Opus, Not Planned.

- [ ] **R118 An element of a temporary container cannot be passed to a read
  parameter**

  Problem: `g(make()[0])` over `def make() -> List[Dup]` is rejected with
  "value of type 'Dup' cannot be implicitly copied", where the pin lends the
  element and destroys the temporary after the call.
  - The element of a named container is lent (`g(xs[0])`).
  - The checker treats the reference result of a temporary receiver as an
    ordinary value read.
  - Workaround: bind the container to a local first.
  - Probe: `conformance/probes/temporary_element_read_argument.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R120 A `mut` capture may be transferred away and written back**

  Problem: `def inner() {mut s} -> String:` whose body runs `var r = s^`,
  `s = String("b")`, `return r^` prints `ab` in Mojito, where the pin reports
  "cannot consume indirect references to values".
  - The pin refuses any `^` out of a capture, written back or not.
  - Mojito treats a capture like a `mut` parameter: left empty it reports
    "'s' is uninitialized at return from this function", written back it runs.
  - The rule belongs in the checker beside `check_transfer_source`.
  - Probe: `conformance/probes/mut_capture_transfer_refilled.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R121 A `TrivialRegisterPassable` struct cannot be transferred out of
  a `mut` parameter**

  Problem: `def take(mut v: V) -> V: return v^` for a
  `struct V(TrivialRegisterPassable)` prints `3 3` at the pin and is rejected
  in Mojito with "'v' is uninitialized at return from this function".
  - The pin transfers a trivial register value as a copy and only warns that
    the `^` has no effect.
  - MIR lowering copies only values whose type owns no storage
    (`owns_droppable_storage`), and every struct counts as owning.
  - Lowering has no conformance facts. The checker knows
    (`is_trivial_register_passable`) and would have to record it on the
    transfer.
  - Probe: `conformance/probes/trivial_struct_mut_parameter_transfer.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R122 A pointer handed to a struct's constructor does not keep its
  pointee alive**

  Problem: `var c = Cell[Pointer[List[Int], ImmOrigin(origin_of(xs))]](p)`
  followed by `print(c.item[][1])` prints 2 at the pin and stops in Mojito
  with "use after Pointer deallocation".
  - The cell takes no loan on `xs`, so `xs` is destroyed after the pointer's
    own last use.
  - A copy read back out (`var q = c.get()`) loses it the same way.
  - A method that stores the pointer after construction does keep it
    (`assets/ok/template_served_loan_carrying_instance.mojo`). A constructor
    body records no transfer, and the caller's aggregate path does not see a
    copied pointer.
  - Probe: `conformance/probes/pointer_copied_into_struct_keeps_loan.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R123 A heap-owning temporary that carries a loan is destroyed twice
  when an owning parameter takes it**

  Problem: `take(Maybe(Span(xs)))` with `def take[T](var m: Maybe[T])` prints
  1 at the pin and stops in Mojito with "use after Pointer deallocation".
  - MIR lowering anchors the temporary in a hidden `$arg_loan_r` slot to keep
    `xs` alive through the call (`anchor_borrowing_argument`,
    `mir/calls.rs`).
  - The slot and the callee's parameter both own the value, and both destroy
    the list inside it.
  - A temporary with no destructor (`Span(xs)`) is unaffected.
  - Probe: `conformance/probes/loan_carrying_temporary_owned_argument.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R124 A struct argument over an immutable origin counts as mutable
  in the exclusivity check**

  Problem: `b.push(s)` with `s: Span[Int, ImmOrigin(origin_of(xs))]` on a
  `Bag` over the same type prints `1 2` at the pin and is rejected in Mojito
  with "aliasing values passed mutably to 'self' argument and passed mutably
  to 'value' argument".
  - The same call over `Pointer[List[Int], ImmOrigin(origin_of(xs))]` is
    accepted.
  - The check reads a struct's origin argument without the `ImmOrigin` cast's
    mutability.
  - Probe:
    `conformance/probes/immutable_origin_struct_argument_exclusivity.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R125 `len` of a list reached through a pointer field does not run**

  Problem: `print(len(c.item[]))` with `c.item` a
  `Pointer[List[Int], ImmOrigin(origin_of(xs))]` prints 3 at the pin and
  stops in Mojito with "vm backend does not support methods on ref yet".
  - The field read yields a reference handle the builtin does not read
    through.
  - Probe: `conformance/probes/len_of_dereferenced_pointer_field.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R126 A clone over an origin binder loses the loan of a copy it
  stores**

  Problem: `s.put(p, 3)` with `def put[U](mut self, value: Self.T, tag: U)`
  storing `value.copy()` prints `3 1 8` at the pin and stops in Mojito with
  "checked nominal subscript receiver is None".
  - A method with a parameter of its own runs as a per-call clone, spelled
    over a clone origin binder where the instance's argument carries a loan.
  - The clone's stored type is closed, so its body publishes no carried
    source, and the copy names no argument expression. The receiver takes no
    loan on `xs`, which is destroyed after its last naming.
  - A generic `def` that keeps its clone at a loan-carrying argument loses
    the loan the same way.
  - A body its template serves keeps it
    (`assets/ok/template_served_def_loan_carrying_argument.mojo`).
  - Probe: `conformance/probes/per_call_clone_copied_loan.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R129 A conditional `__deinit__` is accepted where the struct's
  `Deinitable` conformance is unconditional**

  Problem: `def __deinit__(deinit self) where IsTriviallyDeinitable[Self.T]`
  in `struct Holder[T: Copyable & Deinitable](Copyable, Movable)` runs in
  Mojito, and the pin rejects the struct: "'Holder[T]' does not implement
  all requirements for 'Deinitable' ... lacking evidence to prove
  correctness".
  - The elaborator leaves the `__deinit__` out of `Holder[String]`, whose
    clause fails, so that instance is destroyed without it.
  - The checker should demand that a struct's `Deinitable` conformance be at
    least as conditional as its `__deinit__`, as the pin does.
  - Probe:
    `conformance/probes/conditional_deinit_without_conditional_conformance.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R133 A `Pointer[T, o]` parameter cannot infer both its element type
  and its origin**

  Problem: `def idp[T: AnyType, o: MutOrigin](v: Pointer[T, o])` called with
  `p = Pointer(to=x)` stops with "type mismatch for argument to 'idp':
  expected Pointer[Int, origin#1], found Pointer[Int, origin@…]"; the pin
  runs it.
  - The element binds, but the origin parameter stays an unsolved slot.
  - The same signature with a concrete element (`Pointer[Int, o]`) runs.
  - The parameter order does not matter.
  - Found while fixing a call-result pointer's dereference (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R136 An implicit copy at a binding's last use is not turned into
  a move**

  Problem: `var e = c` where `c` is never read again prints the
  `ImplicitlyCopyable` struct's `copy` in Mojito; the pin prints nothing.
  - The pin transfers a place at its last use instead of copying it; a
    chained comparison's middle operand (`b == d == c` as `c`'s last use)
    shows the same difference.
  - Mojito's ownership analysis knows each last use, but lowering has
    already chosen a copy, and a copy constructor with a side effect makes
    the choice observable.
  - Found while closing the chained-comparison copies (2026-10-03).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R266 A view of a whole local's owned interior is lent as the whole
  local**

  Problem: `var view = s.strip()` then `s = String("q")` then a use of
  `view` is rejected at the store as "access to 's' conflicts with live
  reference 'view'", where the pin rejects the use as "use of invalidated
  interior reference 's["bytes"]'".
  - The verdict agrees; the shape does not: the pin lends `s["bytes"]` and
    the store invalidates it, Mojito lends all of `s`.
  - A field receiver (`h.name.strip()`, `self.name.strip()`) already lends
    its named interior; a bare variable receiver still takes a whole-place
    loan in `aggregate_borrows_unmaterialized`'s view-result path.
  - Moving the owner (`var g = h^`) under such a view reports the stale view,
    where the pin reports "use of uninitialized value 'h'".
  - Found while closing the `self` field store under a live view (2026-10-04).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R302 A linear `Dict.deinit_with` leaves its emptied entries list to
  an implicit destruction its element type does not allow**

  Problem: `Dict[Int, Conn].deinit_with` over a linear `Conn` pops every
  entry and leaves `entries`, an empty `List[DictEntry[Int, Conn]]`, to the
  `deinit self` residual teardown.
  - That list's `__deinit__` is conditional on a `Deinitable` element, so the
    elaborator rightly mints none, and concrete MIR never frees the buffer.
  - The erased run calls the template's destructor and logs a `drop List`
    the concrete run lacks.
  - The checker accepts the implicit residual destruction of a field whose
    conditional `Deinitable` conformance the instance disproves.
  - Consuming `entries` through `List.deinit_with` is the stdlib lever, but
    the nested handler must forward the captured `elt_handler` and move both
    fields out of a `var` entry, which Mojito does not accept yet.
  - `assets/ok/dict_insert_linear_capable.mojo` is the
    `ERASED_VM_RESIDUE` row (`tests/corpus_test.rs`).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R304 A view copied out of a reference result carries no loan on the
  storage it views**

  Problem: `var v = first.value()` over `first = it.peek_next()` of a
  `String`'s codepoint iterator binds a `StringSpan` whose loan on the string
  is lost, so drop elaboration destroys the string first and `String(v)`
  stops with "use after Pointer deallocation"; the pin prints `h`.
  - The binding's loans come from `aggregate_borrows`, which sees no borrow
    for a value copied out of `Optional.value()`'s reference result.
  - Passing the same value straight to `print` works only because a view
    argument stays in its register (`bind_temporary_argument`).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R351 A named `out` result of a type parameter is reported abandoned**

  Problem: `def make[T: Defaultable & Movable](out result: T): result =
  T()` stops with "'result' abandoned without being explicitly destroyed",
  where the pin prints the default value.
  - The result is returned, so it owes no destruction in the callee.
  - A named result of a concrete type runs, on a function and on a method.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R353 A `Pointer` to a tuple element does not bind at the tuple's
  annotated origin**

  Problem: `var p: Pointer[Int, origin_of(t)] = Pointer(to=t[0])` stops with
  "type mismatch for variable 'p'", naming two origins that print alike,
  where the pin prints the element.
  - The pointer's origin is the element's place, which the annotation's
    origin of the whole tuple should admit.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R354 A `deinit self` method reading a `Tuple` field's element beside
  a second `deinit` parameter traps in the VM**

  Problem: `def swap(deinit self, deinit other: Box2) -> Int: return
  self.t[0].id + other.t[1].id`, `t` a `Tuple[Token, Token]` of non-copyable
  elements, stops with "invalid reference projection Field("storage") on
  <moved>", where the pin prints `5`.
  - Each receiver's field is destroyed at its last use, and the element
    read reaches the storage after that.
  - Depends on nothing.
  - Model: Opus, Planned.

### Catch Up To Current Mojo: Calls, Overloads, Conversions, And Traits

Track: `calls`.

Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

- [ ] **R503 A call through a callable local reads a same-spelled
  sibling's slot**

  Problem: two sibling `if c:` blocks binding `var l = one` and
  `var l = two`, each followed by `print(l())`, print `1` and `1`, where
  the pin prints `1` and `2`.
  - In MIR the second `call.indirect`'s callee is `var.use` of the first
    `l`'s slot, while its own `var.store` went to a fresh slot: the callee
    slot is picked by spelling, not by the checked binding.
  - The same two arms in a kept `comptime if n == 0` of a generic `def`
    fail with "'None' is not callable", and in a `comptime for` with a
    `comptime if i == 0` arm they print `1 1`.
  - Found while landing R329 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R510 A user trait named like a prelude trait replaces it**

  Problem: a module declaring `trait Sized: comptime size: Int` prints `1`
  at the pin and stops in Mojito with "struct 'Tuple' declares conformance
  to trait 'Sized' but is missing comptime member 'size'".
  - The user declaration takes the name the bundled library's conformances
    resolve to, where at the pin it shadows the prelude trait in its own
    module alone.
  - Probe: `conformance/probes/user_trait_named_like_prelude.mojo`.
  - Found while planning R488 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R25 A user `Hasher` cannot spell `update` the way the pin requires**

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
    checker's builtin hashable-leaf arm (`method_calls/resolution.rs`), so the call
    side the rename needs exists; `Hasher.update` itself is still the
    checker's intrinsic, not a real `SIMD.__hash__` body.
  - Upstream also replaced the pointer-and-length `hash()` overload with
    `hash_bytes(ImmSpan[Byte])`, which lands in the same pass.
  - The levers are `checker/traits.rs` (the `Hasher` requirement set and its
    shape message), `checker/method_calls/intrinsic_receivers.rs` (the intrinsic arms),
    and `stdlib/std/hashlib/` (`hasher.mojo`, `_ahash.mojo`, `_fnv1a.mojo`
    plus every `hasher.update(...)` call site).
  - It changes a compiler-known trait's contract.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R33 A module-level `def` converts to a `def(...) capturing[_]` runtime
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

- [ ] **R35 A subscript store on a struct with a setter and no getter is
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

- [ ] **R37 A trivial value handed to a `var` parameter beside a pack is always
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

- [ ] **R38 A trivial rvalue handed to a `var` parameter beside a read overload is
  accepted**

  Problem: `q(x + 1)` against `q(var a: Int)` beside `q(a: Int)` is "ambiguous
  call to 'q'" at the pin, while Mojito selects the `var` overload.
  - The pin accepts the literal `q(7)`, the place `q(x)` and the transfer
    `q(x^)` in the same program, so only the non-literal trivial rvalue is
    ambiguous. No rule was recovered.
  - This is the non-variadic sibling of R37, and the two
    disagree: beside a pack the pin calls a bare literal ambiguous, here it
    accepts one.
  - `ArgumentBinding`'s `undecided` bit models the pack rule and is deliberately
    kept off non-variadic candidates, since applying it would reject `q(7)`.
  - Pinned by `conformance/probes/overload_var_trivial_rvalue.mojo`.
  - The plan must find the rule with more probes, or move the entry to
    `docs/non-goals.md` as a kept divergence.
  - Depends on R37, where the `undecided` rule is modelled: this one reuses
    the rule that entry recovers, or is withdrawn with it.
  - Model: Opus, Planned.

- [ ] **R39 A list literal beside a `List` parameter and a pack is not ambiguous**

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

- [ ] **R44 A bound call's result converted at an annotated binding fails
  MIR verification**

  Problem: `var x: Optional[T] = v.copy()` in a generic `def` reports
  "register r1 has no checked type" for the `copy` call, where the pin runs
  it.
  - The same binding at a concrete type runs, and so does the result bound to
    an unannotated local first and then converted.
  - Probe: `conformance/probes/bound_call_result_converted_at_binding.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R45 A matching constructor clone displaces every generic constructor,
  not only its own template**

  Problem: on a struct specialized whole (a variadic one, as `Variant`), a
  call that a per-call constructor clone matches never considers a second
  generic constructor, even one that would rank better.
  - Such a clone is declared `__init__` beside its template, and selection
    drops every generic candidate once a clone matches (`ranks_beside_clone`,
    `checker/declarations.rs`).
  - The clone carries a mark (`MethodProvenance::PerCallConstructor`), not the template
    it was minted from, so the filter cannot drop that template alone.
  - No program is known to hit it. `Variant`'s two generic constructors never
    match the same call, and no fixture pins it.
  - A non-generic struct's clone is minted under its mangled name and reached
    by retargeting, so its overload set holds no clone and needs no filter.
    Minting the specialized struct's clone the same way removes the filter.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R50 A value-returning body that ends in `abort(...)` is rejected**

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

- [ ] **R479 A partial explicit application does not infer the remaining
  parameters**

  Problem: `g2[m: Int, n: Int](a: Box[n])` called as `g2[1](Box[4](0))` is
  rejected with "cannot infer type parameter 'n' of 'g2' from the
  arguments", where the pin prints `5`.
  - The same holds for a lane: `h[DType.int8](Box[4](0), Int8(1))` for
    `h[dt: DType, n: Int](a: Box[n], b: Scalar[dt])`, pin `4`.
  - It holds for a keyword-only prefix too: `k[s="yo2"](Int16(2))` for
    `k[dt: DType, s: String](x: Scalar[dt])`, and `z[T=P](Int8(1))`.
  - Once any parameter is spelled, the checker solves none of the rest from
    the arguments.
  - Found while landing R477 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R52 A binder is not inferred through a converting argument**

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

- [ ] **R382 An `Int` binder is not inferred from a vector width spelled by
  an `Int` expression**

  Problem: `show[w: Int](x: SIMD[DType.int32, w])` called as `show(a)` with
  `a = SIMD[DType.int32, n](1)` in `def f[n: Int]()` is rejected with
  "cannot infer type parameter 'w' of 'show' from the arguments", where the
  pin binds `w` to `n` and prints.
  - `solve_value_args` (`checker/generics.rs`) solves a vector's width slot
    only for a binder declared `SIMDLength`.
  - That is right for a width that is a literal, which the pin leaves an
    `Int` binder unresolved against.
  - A width spelled by an `Int` expression (`n`, an element of a local
    `comptime` display) converts the same way on both sides, so the pin
    solves the binder.
  - Found while landing a display binding read in a type or a parameter
    argument (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R53 A list literal does not reach a `List` built over a binder**

  Problem: `count([1, 2])` for `def count[T: …](extra: List[T])` reports
  "cannot infer type parameter 'T' of 'count' from the arguments", and
  `Bag[Int]([1, 2])` for a fieldwise `var extra: List[Self.T]` reports
  "type mismatch for field 1 of 'Bag': expected List[Int], found List[T]";
  the pin runs both.
  - The literal is typed at the declared `List[T]` without its binder solved
    or substituted, so the field case fails even with `T` given explicitly.
  - Pinned by `conformance/probes/list_literal_to_generic_list.mojo`.
  - Found while deriving another struct's overloaded method; not root-caused.
  - Depends on R52, which lets a binder be solved through a conversion; the
    literal's element type reaches `T` the same way.
  - Model: Opus, Not Planned.

- [ ] **R55 A call through a `ref` to a callable value is rejected**

  Problem: `for f in fns: print(f(5))` and `ref g = fns[0]; print(g(1))` run at
  the pin over a function display, while Mojito reports "'f' has type ref
  def(Int) thin -> Int and is not callable".
  - Both thin and capturing elements are affected.
  - The indexed call `fns[0](5)` already works through element-call dispatch.
    A `ref`-typed callee has no such path.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R56 A method called on a borrowed comprehension binder loses its
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

- [ ] **R58 `Self(...)` does not construct inside a method**

  Problem: `return Self(self.a, not self.b)` in a method of an
  `@fieldwise_init` struct reports "Undefined variable 'Self'", and so does
  the keyword form; the pin constructs the enclosing struct.
  - Naming the struct (`P(...)`, `G(...)` in a generic struct) constructs,
    by position or keyword, so the gap is resolving `Self` as a callee.
  - A static call through `Self` (`Self.plus(a, 1)` in a `@staticmethod`)
    is rejected the same way, where the pin prints `101`; spelling the
    struct (`W[Self.k].plus(a, 1)`) works.
  - Pinned by `conformance/probes/self_call_construction.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R59 Explicit type arguments on a static method of a non-parametric
  struct are rejected**

  Problem: `P.plain[Int](4)` on `@staticmethod def plain[T: Writable](x: T)`
  prints `1` at the pin, while Mojito reports "Undefined variable 'P'".
  - Any generic static on a struct without parameters is affected, whether
    or not its body holds a `comptime if`.
  - The inferred spelling `P.plain(4)` works.
  - The explicit spelling parses as `Invoke` over `Member(P, plain)`. The
    error is raised before the non-parametric static path in
    `checker/method_calls/type_receivers.rs` sees the call.
  - The plan must first find which pass infers the bare type name as a value.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R60 An associated alias is not constructible through a parameterized
  base**

  Problem: `Holder[7].Same()` for `comptime Same = Sized[Self.n]` reports
  `Undefined variable 'Holder'`, where the pin runs it.
  - The annotation spelling works: `var made: Holder[7].Same = Sized[7]()`.
  - The alias now binds the instance's value parameters
    (`associated_type_from_base`), so only the call path is missing.
  - Depends on R59, which finds the pass that reads a bare type name as a
    value; the alias call needs the same answer.
  - Model: Opus, Planned.

- [ ] **R61 A leading-dot static call cannot take its struct's parameters
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

- [ ] **R63 An exact constructor overload loses to its generic sibling as
  ambiguous**

  Problem: `Tag[Int](3)` on a struct declaring `__init__(out self, n: Int)`
  beside `__init__(out self, n: Self.T)` prints `int` at the pin, while
  Mojito reports "invalid call to 'Tag': ambiguous overloaded constructor
  call".
  - Upstream picks the more specific signature. Mojito's construction
    selection scores the substituted `Self.T` parameter as an equal match
    (`select_method_overload`, `checker/declarations.rs`).
  - Every constructor candidate is ranked now, generic or not, so specificity
    can be one more rank term.
  - Such a family is also the one shape that collapses on the instance, so
    its constructor clone family stays withdrawn.
  - Found while closing the overloaded-constructor-family item; no fixture
    pins it yet.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R64 Methods cannot overload on the parameter convention alone**

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

- [ ] **R67 A `mut self` witness does not conform to a read-`self`
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

- [ ] **R81 Ordering a tuple of mixed element types is rejected**

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

- [ ] **R91 Same-arity overloads of a generic method on a generic struct stop
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

- [ ] **R92 A struct's callable-value parameter is undefined inside its
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

- [ ] **R97 `and`/`or` over a non-`Bool` operand is rejected**

  Problem: `n or 2` over `n: Int` prints `2` at the pin, and `if n and True:`
  runs; Mojito rejects both with "operator 'or' is not defined for Int and
  Int".
  - The checker types `and`/`or` only over two `Bool` operands; a numeric
    scalar or a `Boolable` struct never reaches the `Bool(x)` truthiness
    conversion a bare condition now takes.
  - Same-typed operands yield that type at the pin (Python's value-producing
    `or`), so the fix is more than a condition mark.
  - Found while accepting a numeric condition (2026-10-03).
  - A width-1 bool lane (`a == b or c < d` over `UInt64`s) rejects the same
    way, though `if`/`while` accept one through the `Bool(x)` truthiness
    conversion. Workaround: nest the tests.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R102 A method's `*Ts` pack does not take a `Span` argument**

  Problem: `S().show(Span(xs), 1)` over `def show[*Ts: Copyable](self,
  *args: *Ts)` is rejected with "'Span[_]' is not concrete; use '[]' to bind
  missing parameters"; the pin runs it.
  - The same pack on a module `def` takes the span.
  - Once it binds, argument exclusivity already judges the method's
    collected elements, and the pin rejects `S().show(Span(xs), Span(xs))`
    over a `var xs`.
  - `conformance/probes/method_pack_span_argument.mojo` pins it.
  - Found while judging variadic elements in argument exclusivity
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R105 A plain overload beside a variadic one over the same element
  type stops at run time**

  Problem: `def total(by: Int)` beside `def total(*rest: Int)` checks, and
  then `total(3)` fails with "operator Add is not defined for Int and
  Tuple[Int]", where the pin prints both results.
  - Both overloads lower to `total$ov$Int`: the overload key spells a
    collector by its element type, so the MIR holds one function under two
    declarations (`emit-mir` reports "duplicate MIR function name").
  - Struct methods collide the same way; overloads that differ in arity or
    element type, or in a keyword-only name after the collector, do not.
  - The lever is `SignatureKey` (`crates/mojito-symbol/src/symbol.rs`),
    which must mark the collector on the declaration side and at every call
    side that builds a key (`method_lowered_name`, `callable_lowered_name`,
    `callable_contract_target`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R108 The bundled `List` is not `Defaultable`**

  Problem: `Tuple[List[Int], Int]()` is rejected ("constraint declared here
  evaluated to False"), where the pin prints the empty list's length.
  - Upstream's `List` declares `Defaultable`. The bundled one declares an
    empty initializer and not the conformance.
  - `Dict` and `Set` are to be checked against the pin with it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R109 A `Writable` conformance whose only `write_to` witness is
  conditional compiles**

  Problem: `struct Box[T](Writable)` whose `write_to(self, mut writer:
  Some[Writer])` carries `where conforms_to(Self.T, Writable)` is accepted,
  where the pin rejects it ("does not implement all requirements for
  'Writable'").
  - `verify_builtin_conformance` accepts any struct for `Writable`, because
    a struct without a `Writer`-fed `write_to` writes through the reflective
    default.
  - The pin also reports the conditional witness ambiguous beside its
    generated default.
  - This is the one case left where a call through a bound finds no member
    witnessing its requirement (`traits.rs:requirement_witnesses`), so the
    instance is checked again.
  - Found while the ranking fallback of bound-witness selection was removed
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R112 An implicit conversion in a cloned generic method is selected
  again per instance**

  Problem: `var w: Wrapper[Self.T] = self.item` in a method holding a
  `comptime if` is rejected for `Box[Int]` ("ambiguous implicit conversion
  from 'Int' to 'Wrapper[Int]'") when `Wrapper` has `@implicit` constructors
  over `Int` and over `Self.T`, where the pin prints 2 for every instance.
  - The pin selects the constructor while it checks the method, where the
    source is `Self.T`.
  - A method the template serves agrees with the pin
    (`assets/ok/implicit_conversion_bound_on_declaration.mojo`). A method
    that still clones repeats the selection at the instance's types, in its
    clone check and in its derivation.
  - Probe: `conformance/probes/implicit_conversion_bound_on_declaration.mojo`.
  - Found while classifying instance obligations
    (`docs/notes/generator-contract.md`, row 20).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R113 `==` through a bound reaches an `__eq__` over another type**

  Problem: `self.first == self.second` over `T: Equatable` stops in
  elaboration at `Pair[Money]` ("argument 0 of 'Money.__eq__' has type Money,
  declared Cents") when `Money` declares only `__eq__(self, other: Cents)`,
  where the pin prints True.
  - The pin takes `Equatable`'s default, the fieldwise comparison, as the
    witness. The declared `__eq__` serves only a direct `Money == Money`,
    which converts the operand.
  - Mojito synthesizes no default `__eq__`, so the witness is the declared
    one, whose parameter is another type.
  - Until the template served the instance, the clone check converted the
    operand and called the declared `__eq__`, which printed False where the
    pin prints True.
  - Probe: `conformance/probes/equatable_witness_of_another_type.mojo`.
  - Found while the template's MIR replaced per-instance method clones
    (2026-10-01).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R119 A generic constructor beside a declared one cannot be
  constructed on a struct with a value parameter**

  Problem: `P[Int, 3](7)` for `struct P[U: AnyType, n: Int]` declaring
  `__init__(out self, var a: String)` beside
  `__init__[T: Writable](out self, a: T)` prints `5` at the pin and fails in
  Mojito with "vm backend does not support the built-in or callee
  'P.__init__$ov$T$Writable' yet".
  - The same pair runs on a struct whose parameters are all plain type
    parameters (`assets/ok/generic_struct_generic_constructor_overload.mojo`).
  - Such an instance mints the constructor's clone with its other per-call
    clones (`generate_instance_clones`). An instance with a value, origin or
    callable-bounded parameter mints no clones (`instance_arguments`), so the
    request has no owner to mint it.
  - The erased path cannot run it either: `--erased` reports "checked
    constructor 'P.__init__$ov$T$Writable' is missing from MIR".
  - Probe: `conformance/probes/value_struct_generic_constructor_overload.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R127 A method cannot call a generic `def` declared after its
  struct**

  Problem: `def size(self) -> Int: return tally(self.items)` with
  `def tally[T: Copyable & Deinitable](items: List[T]) -> Int` declared below
  the struct prints `1` at the pin and stops in Mojito with "Undefined
  variable 'tally'".
  - The struct may be generic or plain.
  - The same `def` declared above the struct runs.
  - Probe: `conformance/probes/method_calls_later_generic_def.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R393 A `def` cannot call a `def` declared below it**

  Problem: `def a() -> Int: return b(2)` above `def b(n: Int) -> Int` stops
  with "Undefined variable 'b'", where the pin runs it.
  - Module functions are declared in source order, each as its body is
    checked (`check_block`, `checker/statements.rs`).
  - A signature or a field type that applies a function declared below it
    already runs, since the functions a compile-time call applies are
    collected before any declaration (`applicable_functions`,
    `checker/comptime_validation.rs`).
  - Found while landing a function applied in a signature (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R394 A requirement with its own value parameter cannot be called
  through a bound**

  Problem: `x.mk[4]()` in `def use[T: Maker](x: T)`, for `def mk[n:
  Int](self) -> SIMD[DType.int32, n * 2]: ...` in `trait Maker`, fails with
  "MIR function 'use' register r2 names parameter `n` of `Maker.mk` that no
  enclosing declaration binds", where the pin runs it.
  - The call on the conformer itself (`M().mk[2]()`) runs.
  - The result type through the bound keeps the requirement's binder
    instead of the call's argument.
  - Found while landing a function applied in a signature (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R399 A method given a caller's parameter as its own value
  parameter leaves an untyped register**

  Problem: `return g.plain[b]()` in `def user[a: Int, b: Int](g: G[a]) ->
  Int`, for `def plain[m: Int](self) -> Int` in `struct G[k: Int]`, fails
  with "invalid checked program: fn 'user': register r1 has no checked
  type", where the pin runs it.
  - A literal argument (`g.plain[4]()`) in the same `def` runs.
  - The same call on a concrete receiver in `main` (`g.plain[3]()`) runs.
  - The bracket argument is lowered as a read of the binder `b`, and that
    read has no checked type on this path.
  - Found while landing the callee shapes a signature applies (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R128 A `Tuple` over a type parameter needs no evidence the
  parameter is `Movable`**

  Problem: `def total[T: Writable](pair: Tuple[Int, T])` runs in Mojito and
  the pin rejects the signature: "lacking evidence to prove correctness ...
  needs evidence for 'conforms_to(T, Movable)'".
  - `Tuple`'s element bound is `Movable`; a type parameter spelled as an
    element must prove it by its own bounds or a `where` clause.
  - Found while probing closed calls of a generic `def`.
  - Probe: `conformance/probes/tuple_over_unbounded_parameter.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R283 Constructing a type parameter through a user trait's
  `__init__` requirement is unsupported**

  Problem: `return T(v)` in `def build[T: Make & Movable & Deinitable]`,
  where `trait Make` requires `def __init__(out self, v: Int)`, reports
  "unsupported feature", while the pin runs it.
  - The checker constructs a type parameter only through `()` and
    `Copyable`'s `copy=` initializer
    (`Checker::infer_type_param_copy_construction`).
  - The construction should bind its arguments to the bound trait's
    `__init__` requirement, and the elaborator respell it as the bound
    struct's constructor call, as `T(copy=x)` already is.
  - Found while landing `T(copy=x)` (2026-10-04).
  - Probe: `conformance/probes/type_param_trait_init_requirement.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R284 A `@fieldwise_init` initializer does not satisfy a trait's
  `__init__` requirement**

  Problem: `@fieldwise_init struct A(Make, Movable)` with one `var v: Int`
  field reports "declares conformance to trait 'Make' but is missing method
  '__init__'" when `Make` requires `def __init__(out self, v: Int)`, while
  the pin accepts the conformance.
  - The conformance check does not see the synthesized initializer.
  - Found while landing `T(copy=x)` (2026-10-04).
  - Probe: `conformance/probes/fieldwise_init_satisfies_trait_init.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R342 An initializer list with keyword entries, or bound by a
  `comptime` statement, is rejected**

  Problem: `take({1, kw=2})` and `comptime e: Tuple[Int, Int] = {}` are
  valid Mojo, the initializer list being the construction `T(1, kw=2)`,
  where Mojito's parser has no keyword entry in `brace_literal` and the
  elaborator reports `an empty '{}' display needs a Dict[K, V] type
  annotation` for the binding.
  - `{}` and `{a, b}` at a runtime contextual type landed on 2026-10-05
    (`checker/initializer_list.rs`).
  - The parser's `brace_literal` and `comptime/elab.rs` are the two sites.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R495 An initializer list at a struct type over a parameter is
  rejected**

  Problem: `return {0}` in a method returning `It[Self.T]` fails with
  "cannot emit initializer list for 'It[T]': the type has no construction
  to spell", where the pin constructs `It[Self.T](0)`.
  - `checker/initializer_list.rs` spells the construction through
    `ct::source_type`, which has no spelling for a type parameter or a
    value-pack argument.
  - The bundled `ParameterList.__iter__` spells
    `_ParameterListIter[*Self.values](0)` where upstream writes `{0}`.
  - Found while landing R325 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R344 `rebind` cannot change a `Pointer`'s origin**

  Problem: `rebind[Pointer[String, origin_of(a)]](Pointer(to=b))` stops
  with "type mismatch for rebind: the input type does not match the result
  type", naming two origins, where the pin accepts it.
  - Upstream's `Tuple.reverse` and `Tuple.concat` retype their source
    pointers this way.
  - `checker/rebind.rs` compares the closed types whole, origins included.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R409 A template-served method's violated `where` clause is spelled
  from its folded constraint**

  Problem: `v.set[Conn](Conn(4))` on a `Variant[Conn]` whose `Conn` is not
  `Deinitable` reports "expected 'conforms_to(Ts.values, Deinitable)'",
  where the clause is declared, and the pin quotes it, as
  `Ts.all_conforms_to[Deinitable]()`.
  - Both spellings fold to one `GenericConstraint::ConformsPack`, and
    `violated_constraint_reason` (`checker/generics.rs`) prints its
    `Display`.
  - A method the cloner specialized was unparsed from its source
    (`comptime/unparse.rs`), as `Variant`'s were until 2026-10-06.
  - `assets/type_error/variant_set_linear_alternative.mojo` expects only the
    note's prefix until this is fixed.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R420 A keyword call through a bound to a parameter a witness
  renames is rejected**

  Problem: `u.f(x=1, z=2)` through `U: T`, where a conformer declares
  `f(self, z: Int, x: Int)` for the requirement `f(self, x: Int, z: Int)`,
  fails with "binding 'x' of 'f' by name through a trait bound whose
  conformer renames it". The pin binds the keywords by the requirement's
  names and passes the values to the witness by position.
  - Pinned by `conformance/probes/keyword_through_bound_renamed_witness.mojo`
    (the pin prints `312`).
  - The rejection is `Checker::record_bound_default_arguments`
    (`checker/bound_defaults.rs`). It also covers a requirement default the
    check would spell by a renamed name.
  - Cause: a call through the bound reaches the witness by name, in a clone
    re-checked nominally and in `native::mono`'s `infer_call` alike.
  - The lever is to lower such a call positionally, by the requirement's
    binding, which the checked call contract already records.
  - Found while landing R418 with R414 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R419 A trait method cannot declare a `*` or `/` marker**

  Problem: `def f(self, *, k: Int) -> Int` in a trait body fails with
  "positional-only/keyword-only markers on trait methods", where the pin
  accepts it and dispatches `t.f(k=4)` through a bound.
  - The rejection is in `Checker::check_trait` (`checker/traits.rs`).
  - The requirement's keyword-only boundary is already built by the rule a
    struct method uses (`Collectors::of`), so a `*args` collector's
    boundary is modeled; the explicit markers are not.
  - Found while landing R413 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R416 A pack forwarded into a plain struct's own-pack `__init__` is
  rejected**

  Problem: `Sink(*a)` in a pack-keyed `def`, against `def __init__[*Ts:
  Writable](out self, *a: *Ts)`, fails with "no constructor overload
  matches the supplied arguments", where the pin runs it.
  - A direct `Sink(1, "x")` runs, so the constructor's selection does not
    bind a forwarded pack as a method call's does.
  - A constructor spread is a call, not a method call, so serving the
    spreading `def` once it checks is R316's.
  - Found while landing R256 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R437 `o = String(x)` into an `Optional[String]` fails MIR
  verification**

  Problem: `var o = Optional[String](None); o = String(3)` prints `3` at the
  pin, while Mojito stops with "argument 0 of
  'Optional$mono$TString.__init__$ov$T$AnyType' has type StringLiteral,
  declared String".
  - The checker records the `String` wrap of the builtin conversion and the
    `Optional` conversion at the same span, and `implicit_conversions` keeps
    one of them.
  - Workaround: spell `Optional[String](String(3))`.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Catch Up To Current Mojo: Compile-Time Parameters, Packs, And Reflection

Track: `comptime`.

Within the track, an entry Mojito runs to a wrong result, or accepts where the pin rejects, comes first; then one it rejects where the pin runs it; then a verdict that is right with the wrong words.

- [ ] **R527 An explicitly specialized generic `def` converts to a plain function type**

  Problem: `apply(ident[Int], 1)` against `cb: def(Int) -> Int` checks, then fails at run time with "'ident' is not callable"; the pin rejects the conversion.
  - The unspecialized `ident` is already rejected with the pin's message.
  - Reject the specialized form at the call argument the same way.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R531 The non-`Deinitable` field rule covers only a `Tuple` over a pack**

  Problem: a struct field of type `Tuple[*Self.Ts]` now needs a pack bound proving `Deinitable`, as at the pin, but a field of any other struct whose `Deinitable` conformance depends on its arguments is not checked.
  - The rule lives in `checker/traits.rs`.
  - Generalize it to the field type's conditional `Deinitable` conformance.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R526 A capturing generic callable bound to a callable parameter does not elaborate**

  Problem: `invoke_captured[captured_add]()`, where the parameter is `def[n: Int](Int) capturing[origins] -> Int`, fails with "place keeps symbolic type" in `native::mono`; the pin prints 42 (`conformance/fixtures/generic_anonymous_callables.mojo`).
  - The callable is promoted to a runtime parameter that keeps its generic contract type.
  - The body's `callback[2](40)` needs the closure's `n = 2` instance called with the passed environment.
  - Also fails `param_callable_value_test::generic_callable_contract_defaults_override_implementation_defaults`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R528 A default value that does I/O is rejected; the pin evaluates it once**

  Problem: `def f(x: Int = tick())` where `tick` prints is rejected by `native::mono`, where the pin evaluates the default once at compile time.
  - The rejection keeps Mojito a subset; without it each call would print.
  - Evaluate a default once at compile time, as the pin does, then drop the rejection.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R522 Aliasing inside a frozen value is not preserved**

  Two pointer fields into one compile-time allocation freeze as independent copies.
  - `CtValue::Pointer` retains contents and offsets without allocation identity.
  - Preserve an allocation graph when supporting materializable shared storage;
    cyclic memory currently rejects explicitly.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R480 A `comptime if` over an element of a vector-valued binder
  takes the wrong arm**

  Problem: `a5[v: SIMD[DType.int32, 2]]() -> Int32` holding
  `comptime if v[0] > 0: return 7` then `return 9` prints `9` for
  `a5[SIMD[DType.int32, 2](1, 5)]()`, where the pin prints `7`.
  - A plain read of the element (`return v[1]`) is right.
  - `--erased` reports "the erased oracle cannot decide the comptime if
    condition", so the condition is not folded from the binder's value.
  - Found while landing R477 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R508 A body's requested `List` binding cannot be read at compile
  time**

  Problem: `comptime L = mk()` over `def mk() -> List[Int]`, then
  `print(comptime(len(L)))`, prints `3` at the pin and stops in Mojito with
  "vm backend does not support methods on None yet".
  - The binding is a request the elaborator below MIR serves, and the
    compile-time read of it is checked as an ordinary `len` of a value no
    one demanded.
  - A `comptime for` over the same binding works, since its header is the
    demand.
  - Probe: `conformance/probes/requested_list_read_at_compile_time.mojo`.
  - Found while landing R488 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R509 An unread module constant constrained by an applied `where`
  operand is rejected**

  Problem: `comptime C: Int where (f(1) > 5, "big only") = f(1)` with
  nothing reading `C` runs at the pin and stops in Mojito with "unsupported
  generic constraint operand".
  - At the pin a non-generic constrained constant is a generator type,
    checked only where it is used.
  - `Elab::defer_constant` keeps refusing a constrained constant, so it is
    still evaluated where it is declared.
  - Probe: `conformance/probes/constrained_module_constant_unread.mojo`.
  - Found while landing R488 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R518 A generic `comptime` alias whose body applies a callable is
  refused**

  Problem: `comptime Twice[n: Int] = f(n)` applied as `Twice[3]` reports
  "a compile-time call of 'f' is not evaluated above the check", where the
  pin evaluates the alias body in its elaborator and prints `6`.
  - The AST route that evaluated the body is gone (R9); the alias body is
    folded by `Elab::apply_generic_alias` above the check, which folds no
    call.
  - The lever is the one the module constants took: the check binds the
    application as a parameter expression and `native::mono` evaluates it.
  - A struct's associated `comptime` member that applies a callable
    (`comptime k = f(1)` inside a struct) was already refused before R9
    ("not an associated comptime expression") and takes the same lever.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R519 A method of an unbound pack other than `__len__` has no
  symbolic rule**

  Problem: `a.__getitem__[0]()` over `*a: *Ts` is rejected with "no symbolic
  rule for an unbound pack or reflected type: method '__getitem__' of an
  unbound pack", where the pin runs it; `a[0]` is accepted.
  - Source validation left such a body without a verdict for a
    per-instantiation check that no longer exists; the one check judges
    every body with its binders symbolic
    (`tests/checker_test.rs::a_pack_body_without_a_symbolic_rule_is_rejected`).
  - The lever is a rule for the explicit dunder spelling beside the
    subscript's (`Checker::pack_element_type`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R471 A local `comptime` struct binding in a plain `def` is stored
  and destroyed**

  Problem: `comptime c = N(8)` then `print(c.a)` twice in `main`, over a
  `struct N` with a printing `__deinit__`, prints `8 / 8 / del 8` in
  Mojito, where the pin prints `8 / 8` and never destroys a `N`.
  - MIR gives `c` a frame slot that holds the frozen value and drops it, as
    if it were a `var`.
  - At the pin the binding is a parameter: a field read is a constant, and
    only a run-time use of the whole value materializes a temporary.
  - In a generic `def`, a value parameter's field read already folds.
  - Found while landing R467 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R472 A plain `def`'s `comptime for` element read through a `String`
  field materializes the whole element**

  Problem: `comptime for e in [Q(1, In(2), 1.5, "x")]:` then `print(e.s)`
  in `main` builds and destroys a `Q`, printing `del In 2 / del Q 1`, where
  the pin reads the field alone.
  - The AST unroller substitutes the element's construction for `e`, and
    folds a field chain to its leaf only when the leaf's literal has the
    field's type (`comptime/rewrite.rs::materializes_at_its_type`).
  - A `String`, float, `UInt`, or tuple leaf would materialize as a literal
    of another type, so `e.s` keeps reading the whole element.
  - An `Int`, `Bool`, `DType`, vector, or struct leaf folds (`e.a`,
    `e.i.v`, `show(e.i)`).
  - Found while landing R467 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R481 A value binder typed by a lane type is rejected**

  Problem: `s[dt: DType, v: Scalar[dt]]() -> Scalar[dt]` called as
  `s[DType.int16, 5]()` is rejected with "type mismatch for value parameter
  'v': expected Scalar[dt], found 5", where the pin prints `6`.
  - The argument is checked against the declared type before the earlier
    binder `dt` is substituted into it.
  - Found while landing R477 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R482 A sized-scalar value binder is rejected**

  Problem: `s[v: Int16]() -> Int16` is rejected with "unknown trait 'Int16'
  in a type-parameter bound", where the pin prints `6` for `s[5]()`.
  - The capitalized name in the binder's annotation is read as a trait
    bound, not as a value type.
  - Its argument `s[Int16(5)]()` also stops the parser with "Expected ']'
    after a subscript", a different shape from R381's operator.
  - Found while landing R477 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R483 A method's explicit value parameter supplied from the caller's
  own binder fails MIR typing**

  Problem: `s.m[n](a)` or `S.st[n](a)` inside `def by_value[n: Int](a:
  Float64)`, against `def m[n: Int](self, a: Float64) -> Int`, fails with
  "register r4 has no checked type (UseVar …)", where the pin prints the
  method's result.
  - The same argument as a literal (`s.m[5](a)`) runs, and so does a free
    `def`'s `k[n](a)` from the same body.
  - MIR emits the binder as a runtime register, but the checker records no
    type for the argument expression on the method path.
  - Found while landing R478 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R289 An explicit type argument before an inferred pack binds the
  pack empty**

  Problem: `has[Int](7, "x", False)` against `def has[T: AnyType, *Us:
  Writable](*extra: *Us)` fails with "'has$mono$TInt$T$pack$' expects 0
  argument(s), got 3", while the pin prints the body's result.
  - The instance key names `T` and an empty pack: the explicit argument
    fills `T`, and the pack the call's arguments imply is never solved.
  - With `T` inferred from an argument (`has(1, 7, "x")`) the call runs.
  - Found while landing R62 (2026-10-04).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R319 A local type alias applied as a constructor in an unrolled
  `comptime for` body is an undefined variable**

  Problem: `comptime T = Int` then `print(T(3))` inside `comptime for i in
  range(2)` in a plain `main` fails with "Undefined variable 'T'", where the
  pin prints `3` twice.
  - The same alias used as an annotation works, and the same call outside a
    loop works, so the AST unroller (`Elab::unroll_comptime_for`) loses the
    alias's binding for a call position.
  - Found while planning R246 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R502 A parametric `comptime` alias in a function body is rejected**

  Problem: `comptime A[k: Int] = k + 1` inside a `def`, read as `A[10]`,
  fails with "a generic comptime alias must be declared at module scope",
  where the pin prints `11`.
  - The same alias at module scope runs.
  - Probe: `conformance/probes/local_generic_comptime_alias.mojo`.
  - Found while landing R310 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R475 A local type alias over `Scalar` or `SIMD` is rejected**

  Problem: `comptime S = Scalar[DType.float64]` then `S(2) * 1.25` in a
  plain `main` fails with "not a compile-time value: 'Scalar' is not a
  compile-time type", where the pin prints `2.5`.
  - In a lane-keyed `def`, `comptime S = Scalar[dt]` fails with "Undefined
    variable 'Scalar'" and `comptime S = SIMD[dt, 1]` with "Undefined
    variable 'S'".
  - The elaborator's `Elab::type_from_name` (`comptime/elab.rs`) has no
    type for a parametric alias or `SIMD`, and the checker's
    `comptime_type_operand` does not bind one either.
  - Found while landing R261 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R476 `comptime k = Self.n` in a generic struct's method is
  rejected**

  Problem: `comptime k = Self.n` (a bare member) in a method or static
  method of `struct S[n: Int]` fails with "type 'S[n]' has no associated
  type 'n'", where the pin prints `4` for `S[4]`.
  - `comptime_type_operand`'s `Self.` arm
    (`checker/comptime_validation.rs`) takes the member for a type.
  - R312 covers `Self.n + 1` cloning per instance; this is a rejection.
  - Found while landing R261 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R402 A plain `def`'s `comptime for` over struct elements built by a
  call is rejected**

  Problem: `comptime for p in [mk(3), P(1)]:` in a non-generic `def` fails
  with "VM CTFE failed for 'mk': ... 'comptime for' over elements of type
  'P' in a generic body", where the pin prints each element.
  - The CTFE program for `mk(3)` still holds the enclosing loop, not yet
    unrolled, and its check rejects a loop variable that is no binder
    (`check_comptime_for`, `checker/comptime_validation.rs`).
  - The same loop in a generic `def` runs, on a clone.
  - Probe: `[mk(3), P(1)]` over `@fieldwise_init struct P` with
    `def mk(n: Int) -> P`.
  - Found while landing R363 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R443 An initializer list at a reflected field type is rejected**

  Problem: `var x: types[i] = {}` over `comptime types =
  reflect[T].field_types()`, under a `conforms_to` arm proving
  `Defaultable`, fails with "cannot emit initializer list for
  'reflect[T].field_types()[i]': the type has no construction to spell",
  where the pin prints each field's default.
  - `Checker::initializer_list_construction`
    (`checker/initializer_list.rs`) rewrites a brace into a spelled callee,
    and a reflected element has no name to spell.
  - The lever: have the brace record `SemanticAdjustment::ConstructType`,
    which MIR already lowers to `MirInstr::ConstructType`, instead of
    spelling a call.
  - `types[i]()` and `x: types[i] = types[i]()` run
    (`assets/ok/reflection_field_type_construction.mojo`).
  - Found while landing R364 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R444 A reflected field type over a closed subject in a plain `def`
  cannot be constructed**

  Problem: `comptime types = reflect[Holder].field_types()` in `main`, then
  `types[i]()` or `FT()` under `comptime if conforms_to(types[i],
  Defaultable & ...)` inside `comptime for i in
  range(reflect[Holder].field_count())`, fails with "type
  'type_sequence[Int, Float64][i]' does not conform to trait
  'Defaultable'", where the pin prints each field's default.
  - Source validation types the element of the closed list as a selection
    (`ParamKind::Select`), and `Checker::opaque_element`
    (`checker/comptime_validation.rs`) gives the arm's proof only to a pack
    element or a reflection over a parameter.
  - Past that, the AST unroller leaves `types[0]()` naming a dropped
    `comptime` binding ("Undefined variable 'types'"), the call-position
    loss R319 describes for an alias.
  - The same loop in a generic `def` runs, served by its template.
  - Found while landing R364 (2026-10-07).
  - Depends on R319, which loses a local alias at a call in an unrolled
    body.
  - Model: Opus, Not Planned.

- [ ] **R501 A struct method cannot iterate a module `comptime` list a call
  builds**

  Problem: `comptime ML = mk(3)` at module scope, then `comptime for x in
  ML:` in a struct's method, stops with "function instantiation in
  parameter domain that recursively requires itself: the initializer of
  'ML' reads 'ML'", where the pin prints `0`, `1`, `2`.
  - The same loop in a `def` runs, and so does a literal module list, so
    the cycle the message names is a false one.
  - Probe: `conformance/probes/module_call_list_in_method_loop.mojo`.
  - Found while landing R310 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R368 A module `comptime` constant declared after the `def` that
  iterates it is undefined**

  Problem: `def f(): comptime for x in L: print(x)` followed by `comptime L
  = [10, 20]` fails with "Undefined variable 'L'", where the pin prints
  `10`, `20`.
  - The elaborator evaluates module constants in source order
    (`Elab::top_consts`), so the loop meets a name it has not bound yet.
  - With the constant declared first the loop runs, in a generic `def` too.
  - Found while landing R246 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R376 A compile-time application under an untaken `comptime if` arm
  is evaluated**

  Problem: `comptime if n > 100:` holding `comptime for z in [10 // (n -
  4)]:` in `def f[n: Int]()` stops `f[4]()` with "integer division or modulo
  by zero", where the pin skips the arm and runs.
  - `native::mono` unrolls every loop header of an instance before it
    selects a branch (`unroll_comptime_loops`, then
    `select_comptime_branches`, `mono/specializer.rs`), so the header's
    thunk runs in an arm the instance never takes.
  - A nested `comptime if` whose condition applies a function fails the
    same way.
  - A local `comptime` display binding read in such an arm does too.
  - Found while landing reads of a local display binding (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R377 `materialize[L[1]]()` over a capitalized name is parsed as a
  type application**

  Problem: `comptime L = [1, 2]` then `print(materialize[L[1]]())` in `main`
  is rejected with "materialize[...]() takes one compile-time value", where
  the pin prints `2`.
  - The parser reads `L[1]` in a parameter argument as the type `L` applied
    to `1`, since the name is capitalized.
  - A lower-case name (`vals[1]`) is parsed as a subscript and runs.
  - Found while landing reads of a local display binding (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R381 A capitalized subscript followed by an operator does not parse
  in a parameter argument**

  Problem: `comptime L = [n, n * 2]` then `SIMD[DType.int32, L[0] * 2](5)`
  in `def f[n: Int]()` stops with "Expected ']' after a subscript", where
  the pin prints an eight-lane vector for `f[4]()`.
  - The parser reads `L[0]` in a parameter argument as the type `L` applied
    to `0`, since the name is capitalized, and a type takes no operator.
  - The bare `L[0]` is accepted: the check and MIR read the type
    application as the element (`display_element_argument`).
  - A lower-case name (`vals[0] * 2`) is parsed as a subscript and runs.
  - Found while landing a display binding read in a type or a parameter
    argument (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R380 Two local `comptime` bindings of one display are different
  values in a type**

  Problem: `comptime L = [n, n * 2]` and `comptime M = [n, n * 2]`, then
  `var a: SIMD[DType.int32, L[0]] = SIMD[DType.int32, M[0]](1)`, in `def
  f[n: Int]()` is rejected with "type mismatch for variable 'a'", where the
  pin runs it.
  - The check denotes each binding by the application of its own lifted
    function, named by the binding (`record_display_binding`,
    `checker/comptime_validation.rs`), so `L[0]` and `M[0]` are two
    expressions.
  - The pin identifies the two reads by structure. It still tells `L[0]`
    from `n`, which `assets/type_error/comptime_display_element_identity.mojo`
    pins for `L[0] * 2` against `L[1]`.
  - Found while landing a display binding read in a type or a parameter
    argument (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R390 Two spellings of an application are one value only when
  their names resolve to the same bindings**

  Problem: `comptime vals = [n, n * 2]` and `comptime other = [n, n * 2]`,
  then `var a: SIMD[DType.int32, h(vals[0])] = SIMD[DType.int32,
  h(other[0])](1)`, in `def f[n: Int]()` is rejected with "type mismatch for
  variable 'a'", where the pin runs it.
  - The check gives one lifted function to two occurrences of an expression
    when their syntax is equal and each name resolves to the same binding
    (`Checker::lifted_application`, `checker/comptime_validation.rs`).
  - The pin identifies the two by the structure of what they compute.
  - A tuple binding is the second case: `comptime t = (n, n * 2)` then
    `h(t[0])` outside a `comptime for` and inside it are two functions,
    since the binders a value that denotes no parameter expression reads
    are not known (`Checker::binders_read`).
  - The check still tells `h(n)` from `n * 2`, which
    `assets/type_error/comptime_application_identity.mojo` pins.
  - Found while landing an application in a type or a parameter argument
    (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R395 A struct's `Bool` parameter read as a value is typed `Int`**

  Problem: `def on(self) -> Bool: return Self.b` in `struct Flag[b: Bool]`
  is rejected with "type mismatch for return: expected Bool, found Int",
  where the pin prints `True`.
  - `self.x = 1 if Self.b else 0` passes the check and runs on the VM. The
    native backend rejects it with "Op has different operand types".
  - A `Bool` parameter of a `def` read the same way is a `Bool`.
  - Found while landing a function applied in a signature (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R386 A parameter argument that computes a `String` or a float from
  a parameter is rejected**

  Problem: `label[pick(n)]()` for `def label[s: String]()` and
  `scale[half(n)]()` for `def scale[x: Float64]()`, in `def f[n: Int]()`,
  are rejected with "not a compile-time Int constant: not an associated
  comptime expression", where the pin prints both.
  - The check lifts an `Int` or a `Bool` expression only
    (`Checker::lifted_application`, `checker/comptime_validation.rs`).
  - A lifted function returning a `String` or a `Float64` already serves a
    `comptime for` display, so the elaborator can freeze either value.
  - The parameter domain needs a value meta for each, and `UInt` and the
    sized integers want the same.
  - Found while landing an application in a type or a parameter argument
    (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R489 An associated `comptime` member that applies a function is
  rejected**

  Problem: `comptime V = f(Self.k)` in `struct S[k: Int]`, read as
  `Self.V`, prints at the pin, while Mojito reports "not a compile-time Int
  constant: not an associated comptime expression".
  - A function body's local binding of the same call is a request the
    elaborator serves per instance (`docs/notes/ctfe-request-path.md`).
  - The member is the generator's, evaluated under the instance's bindings,
    so it can denote the same application.
  - Found while planning R7 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R396 A signature that calls a function over a value that is no
  `Int` or `Bool` is rejected**

  Problem: `def gen[n: Int]() -> SIMD[DType.int32, Int(half(n))]:` for `def
  half(x: Int) -> Float64`, and `-> SIMD[DType.int32, P(n).get()]` for a
  method `get` of `struct P`, are rejected with "not a compile-time Int
  constant: not an associated comptime expression", where the pin runs both.
  - A call is the application of its callee by name only when every
    parameter and the result is an `Int` or a `Bool`
    (`applicable_functions`, `checker/comptime_validation.rs`).
  - A method call's receiver is a struct value, which is no such argument.
  - In a body each of these is lifted whole, so it runs there, as a
    different value from the same call spelled in another declaration.
  - A struct value in the parameter domain can take the frozen form a
    struct-typed parameter argument already has.
  - Found while landing the callee shapes a signature applies (2026-10-06).
  - Depends on R386, which gives the parameter domain a value meta for a
    `Float64` and a `String`.
  - Model: Fable, Not Planned.

- [ ] **R397 A signature that calls a function generic over a type is
  rejected**

  Problem: `def gen[n: Int]() -> SIMD[DType.int32, idt(n)]:` for `def
  idt[T: Copyable](x: T) -> T` is rejected with "not a compile-time Int
  constant: not an associated comptime expression", where the pin runs it.
  - A callee's own compile-time parameters are applied by name when each is
    an `Int` or a `Bool` the call spells (`twice[n]()`).
  - A type parameter, a parameter the call leaves to inference, and one
    with a default are left out (`ApplicableShape::applicable`,
    `checker/comptime_validation.rs`).
  - A default argument that reads a name the calling declaration binds too
    is left out as well, since the default belongs to the callee's scope
    (`Checker::applied_default`).
  - In a body such a call is lifted whole, so it runs there.
  - Found while landing the callee shapes a signature applies (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R387 A function applied in a parameter argument cannot print**

  Problem: `g[noisy(n)]()` in `def f[n: Int]()`, where `noisy` prints before
  it returns, is rejected with "is not safe for VM-backed compile-time
  execution: it reaches `print`", where the pin prints once while compiling
  and runs.
  - The elaborator refuses to run a lifted function that reaches `print` or
    `input` (`effectful_callee`, `mono/specializer.rs`).
  - The pin's interpreter writes compile-time output when the instance is
    elaborated.
  - Decide where an elaboration-time `print` writes on each backend before
    lifting the refusal.
  - Found while landing an application in a type or a parameter argument
    (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R388 A type over a lifted application is spelled by its function's
  name in a diagnostic**

  Problem: `var a: SIMD[DType.int32, max(n, 2)] = SIMD[DType.int32, n *
  2](1)` in `def f[n: Int]()` is rejected with "expected SIMD[DType.int32,
  $comptime$$2014(n)], found SIMD[DType.int32, 2 * n]", where the pin spells
  the expected type `SIMD[.int32, max(n, Int(2))]`.
  - Both compilers reject the program. Only the spelling differs.
  - The application names the function MIR lifts, not the expression it
    computes (`Checker::lifted_application`,
    `checker/comptime_validation.rs`).
  - A call of a plain module `def` is spelled as written (`h(n)`), since it
    is the application of that `def`.
  - Found while landing an application in a type or a parameter argument
    (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R398 A type over the application of a generic, overloaded, or
  static callee is spelled by its symbol in a diagnostic**

  Problem: `var x: SIMD[DType.int32, 4] = gen[2]()` for `def gen[n: Int]()
  -> SIMD[DType.int32, twice[n]()]` is rejected with "found
  SIMD[DType.int32, twice(2)]", where the pin spells the found type
  `SIMD[.int32, twice[Int(2)]()]`.
  - Both compilers reject the program. Only the spelling differs.
  - An overloaded callee prints its symbol: `ov$ov$Int(2)` for `ov(2)`.
  - A static method of a generic struct prints the instance it is called on
    as an argument: `G.w[G[2]]()` for `G[2].w()`.
  - `not (n > 9)` prints as `not 9 < n`.
  - The application holds the MIR declaration's name and every argument in
    one list (`ParamKind::Apply`, `param_expr.rs`), so the printer knows
    neither the source name nor which arguments are compile-time ones.
  - Found while landing the callee shapes a signature applies (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R512 A subscript of a module list or tuple constant in a parameter
  argument is rejected**

  Problem: `show[XS[1]]()`, `show[T[1]]()`, and `SIMD[DType.int64,
  xs[1]](7)` over module constants `comptime XS = [1, 4, 5]` and
  `comptime T = (2, 3)` are rejected, where the pin runs all three.
  - A capitalized constant (`XS[1]`, `T[1]`) parses as a type application
    and is rejected with "type mismatch for value parameter 'n': expected a
    value, found a type", or "SIMD width … got a type".
  - A lowercase one (`xs[1]`) is rejected with "not a compile-time Int
    constant: xs".
  - It does not matter whether the constant is folded where it is declared
    or waits for its first demand.
  - R383 reports the local display binding's spelling of the same parse.
  - Found while deferring display and subscript module constants
    (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R383 A vector width that mismatches a display element is reported as
  a missing constant**

  Problem: `comptime vals = [n, n * 2]` then `var a: SIMD[DType.int32,
  vals[0]] = SIMD[DType.int32, n](1)` in `def f[n: Int]()` is rejected with
  "not a compile-time Int constant: vals", where the pin reports that
  `SIMD[.int32, n]` does not convert to the annotated type.
  - Both compilers reject the program. Only the diagnostic differs.
  - Source validation reports it, on the conversion it tries after the two
    types differ.
  - The capitalized spelling `L[0]` reports "SIMD width must be a positive
    power of two, got a type" instead.
  - Found while landing a display binding read in a type or a parameter
    argument (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R448 A value that solves a type parameter to `String` is rejected**

  Problem: `f["lit"]()` and `f[String("s")]()` over `def f[T:
  ImplicitlyCopyable & Writable, //, v: T]() -> T` fail with "value
  parameter 'v' typed by 'T' cannot bind a 'String' value yet", where the
  pin solves `T = String` and prints `lit` and `s`.
  - A compile-time string is a `CtValue::Str`, typed `StringLiteral`, so a
    read of `v` at `T = String` would put a literal in a `String` register.
  - Mono must close the read as the literal followed by the conversion MIR
    already uses for a `[s: String]` read; the checker then accepts the
    `Str` against the `String` struct in one shared rule.
  - The rejection lives in `Checker::solve_binders_from_values`.
  - Found while landing R446 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R449 A keyword-supplied infer-only parameter is rejected**

  Problem: `f[T=Int, n=3](1)` over `def f[T: Writable & ImplicitlyCopyable,
  //, n: Int](x: T) -> Int`, and `PL[T=Int]()` over `struct PL[T: …, //,
  *vs: T]`, fail with "infer-only parameter 'T' cannot be supplied
  explicitly", where the pin accepts both and prints `3` and `0`.
  - The pin rejects only a positional argument for an infer-only binder.
  - The check is made twice: in `Checker::resolve_use_params` and in
    `split_callable_specialization` (`checker/generics.rs`).
  - Found while landing R446 (2026-10-07).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R450 A `UInt` value parameter inferred from an argument type is missing at the call**

  Problem: `name(S[UInt(3)]())` over `def name[v: UInt](s: S[v]) -> UInt`
  fails with "required compile-time value parameter 'v' is missing", where
  the pin prints `3`; the same holds for `v: T` solved to `UInt`.
  - MIR's `inferred_param_arg_regs` passes an inferred `Int`, `Float64`,
    `Bool`, string, or `DType` value as a constant register, but has no
    constant for a `UInt`.
  - Mono already reads a bound `UInt` as its 64 bits in a `UInt` slot, which
    the register needs to match.
  - Found while landing R446 (2026-10-07).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R451 The elaborator classifies a value parameter whose type it cannot read as a type parameter**

  Problem: `classify_ct_param` (`comptime/params.rs`) makes `v` in `def
  pick[v: Self.T](self)` a type parameter bounded by "trait `Self.T`", and
  does the same for a value type it cannot read (`items: List[T]`), while
  the checker classifies both as value parameters.
  - Programs run today because a template-served method binds its value
    from the call's recorded arguments, not from this classification.
  - The method form needs the enclosing struct's binder identity, which
    `classify_ct_param` is not given.
  - Found while landing R446 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R490 A call that leaves a value pack empty is rejected**

  Problem: `total()` and `total[]()` over `def total[*values: Int]() ->
  Int` fail with "cannot infer type parameter '*values' of 'total' from the
  arguments" and "an empty subscript ('value[]') is the pointer
  dereference", where the pin binds the empty list and prints `0`.
  - The parser reads `total[]` as the pointer dereference `p[]`, so the
    call must tell a generic callee's empty brackets from it.
  - The checker's generic call binding has no empty list for an unsupplied
    variadic value parameter.
  - A clone of such a `def` would spell its runtime reads as
    `ParameterList()`, whose infer-only `type` then has no value to solve
    from (R449 covers the keyword form).
  - Found while landing R325 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R452 A `comptime` alias of an applied generic function is rejected**

  Problem: `comptime g = f[7]` then `g()`, over `def f[v: Int]() -> Int`,
  fails with "not a compile-time value: 'f' is not a compile-time type",
  where the pin binds the specialized function and prints `7`.
  - The alias path evaluates the right-hand side as a type application.
  - With a type argument (`f[Int, 7]`) it fails with "unknown type 'f'".
  - Found while landing R446 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R464 A compile-time value of a struct without fieldwise
  construction is rejected**

  Problem: `comptime r = R(3)` over a `struct R` (`a: Int`, `s: String`)
  whose only `__init__(out self, a: Int)` sets `self.s = String(a)` fails
  with "a compile-time 'R' value needs fieldwise construction", where the
  pin prints `r.s` as `3`.
  - The VM freezes the result (`VmBackend::freeze`), but
    `Elab::check_frozen_structs` (`comptime/ctfe.rs`) refuses it, since a
    frozen `CtValue::Struct` comes back as a call to the fieldwise
    constructor.
  - Mojo materializes the parameter attribute directly, with no
    constructor call.
  - Found while landing R456 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R465 A compile-time construction of a generic struct is rejected**

  Problem: `comptime g = G[Int](5, "gen")` over `@fieldwise_init struct
  G[T: Copyable & Movable & Writable & Deinitable]` fails with "'G' is not
  a compile-time-callable function", where the pin prints `5 gen`.
  - The same happens without the `String` field.
  - The elaborator routes a parameterized callee to CTFE of a function by
    that name rather than to `Elab::ctfe_struct_entry`.
  - Found while landing R456 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R461 A `String(...)` construction in a parameter argument is
  rejected**

  Problem: `g[String("a")]()` for `def g[s: String]()` and
  `g[(1, String("a"))]()` for `def g[p: Tuple[Int, String]]()` fail with
  "not a compile-time Int constant: not an associated comptime expression",
  where the pin prints `a` and `1 a`.
  - `Checker::eval_associated_ct` does not evaluate a `String` construction,
    and the lifted fallback serves `Int` and `Bool` only.
  - The literal spellings (`g["a"]`, `g[(1, "a")]`) run.
  - R386 is the same gap over an open parameter (`label[pick(n)]`).
  - Found while landing R455 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R462 A tuple argument over an enclosing binder is rejected**

  Problem: `g[(n, 2)]()` inside `def f[n: Int]()`, against `def g[p:
  Tuple[Int, Int]]()`, fails with "type mismatch for value parameter 'p':
  expected Tuple[Int, Int], found (n, 2)", where the pin prints `5 2` for
  `f[5]()`.
  - The argument stays a symbolic `CtValue::Expr`, which `ct_value_ty` has
    no tuple type for, and contextual typing is tried only for a closed
    value (`Checker::converts_to_parameter`).
  - A scalar argument over the binder (`h[n + 1]`) runs.
  - Found while landing R455 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R463 A struct's tuple-typed value parameter cannot be read in its
  methods**

  Problem: in `struct T[p: Tuple[Int, Int]]`, `var t = Self.p` fails with
  "no field 'p'" and `Self.p[1]` with "Undefined variable 'Self'", where
  the pin prints both elements of `T[(1, 2)]`.
  - A `Tuple[Int, String]` parameter fails the same way.
  - A struct's scalar or struct-typed value parameter reads (`Self.n`,
    `Self.e.rows`).
  - Found while landing R455 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R326 An empty explicit application `f[]()` is a pointer dereference**

  Problem: `none[]()` over `def none[*vals: Int]() -> Int` fails with "an
  empty subscript ('value[]') is the pointer dereference", where the pin
  binds the pack empty and prints `0`.
  - The parser reads `name[]` as `EmptySubscript` whatever `name` denotes;
    the checker would type it as an application when the callee is a
    generic `def`.
  - Found while landing R318 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R313 A generic struct's method that still clones per instance has
  no clone for a `StringLiteral` instance**

  Problem: `Box("a").local_ty()`, on a `Box[T]` whose `local_ty` binds
  `comptime U = Self.T`, stops with "abort: Box.local_ty: unspecialized
  type-keyed method", while the pin prints `1 0` beside `Box(1)`.
  - The receiver is typed `Box[StringLiteral]`, and MIR holds the `Int`
    instance's clone of `local_ty` but none for `StringLiteral`, so the call
    reaches the template's stub; `Box(String("a"))` runs.
  - Only a method R312 still clones can hit it.
  - Probe: `conformance/probes/stub_method_string_literal_instance.mojo`.
  - Found while landing R305 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R296 A method local annotated with a struct pack element's type is
  rejected**

  Problem: `var value: Self.Ts[i] = Self.Ts[i]()` in a method's `comptime
  for` fails with "dependent type indexing requires a type-valued associated
  member" (`checker/type_resolution.rs`), while the pin prints each
  element's default.
  - A `def`'s own `Ts[i]` annotation resolves to the dependent element
    (`assets/ok/pack_element_alias_served.mojo`); the `Self.Ts[i]` spelling
    has no such form.
  - Found while landing R252 (2026-10-04).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R65 Upstream `DType` names with no Mojito dtype are rejected**

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
  - The bundled `struct DType` port, R160, rewrites the same table, so this
    lands first or folds into it.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R332 A `DType` float query as a type argument is rejected**

  Problem: `SIMD[DType.int32, DType.exponent_width[DType.float32]()](2)`
  prints its vector at the pin, while Mojito reports "not a compile-time Int
  constant: not an associated comptime expression".
  - The same query over a `DType` binder (`SIMD[DType.int32,
    DType.exponent_width[dt]()]` in `def lanes[dt: DType]()`) rejects the
    same way.
  - `eval_associated_ct` and `compile_dependent_ct_expr`
    (`checker/constraints.rs`) have no arm for the query. A closed one is
    its answer, and one over a binder is the application
    `ParamContext::dtype_float_query` builds, which the elaborator answers
    per instance.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R333 A value's `dtype` as a `DType` query argument is rejected**

  Problem: `DType.exponent_width[x.dtype]()` for `x: Scalar[dt]` prints `8`
  at `Float32` at the pin, while Mojito reports "not a valid SIMD element
  type: DType.dtype".
  - `dtype_from_arg` (`checker/annotations.rs`) reads only a
    `DType.<name>` spelling, a binder, or a type's `dtype`, not a value's.
  - A value's `dtype` is its type's lane slot, which the checker already
    records for a runtime read (`SemanticAdjustment::DtypeConstant`).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R337 A computed value argument in a bracket is rejected**

  Problem: `W[fib(6)]()` for `struct W[n: Int]`, or `g[fib(6)]()` for `def
  g[n: Int]()`, prints `8` at the pin, while Mojito reports "not a
  compile-time Int constant: not an associated comptime expression".
  - `eval_associated_ct` (`checker/constraints.rs`) has no arm for a call;
    a module constant over the same call (`comptime N = fib(6)`) folds
    before the check and works.
  - The pin keeps such an application symbolic through the check
    (`ParamKind::Apply`), so it is no frozen literal; a struct-typed
    argument is frozen to its fieldwise construction
    (`Elab::freeze_struct_value_arguments`), which a scalar one should not
    copy.
  - Found while landing R4's struct-valued half (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R338 A static call through a `comptime` alias of a generic struct is
  rejected**

  Problem: `comptime W3 = W[3]` then `W3.total()`, a `@staticmethod` of
  `struct W[n: Int]`, prints `3` at the pin, while Mojito reports "function
  types as compile-time values".
  - The receiver reaches `infer` as a `TypeValue`, which has no
    expression type; a construction through the alias (`W3()`) works.
  - Found while landing R4's vector-keyed half (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R66 Small SIMD surface gaps the symbolic-lane probes found on concrete
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

- [ ] **R68 An imported alias in a cloned generic method's signature is an
  unknown type**

  Problem: `def feed(self, mut hasher: default_hasher)` on a generic struct
  reports "unknown type '__module$hasher$default_hasher'" for an instance
  that clones the method, while the pin runs it.
  - A method clones where its body holds a compile-time construct, or its
    instance's argument carries a loan. A method the template serves
    resolves the alias.
  - The same annotation resolves on a module-level `def`, on a plain
    struct's method, and through a local `comptime` alias of the import.
  - Workaround: spell the application (`AHasher[SIMD[DType.uint64, 4](0)]`),
    as `assets/ok/template_method_bound_witness_shapes.mojo` does.
  - Pinned by `conformance/probes/imported_alias_in_generic_method.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R69 An uncalled `rebind` method is checked for every instance**

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

- [ ] **R458 A generic `def` whose clone fails to elaborate is rejected
  even when no reachable call needs it**

  Problem: `f[5]()` inside an uncalled `def unused()`, over a `def f[n: Int]`
  whose `comptime q = [1, 2][i]` runs past the display at `n = 5`, is refused
  with "comptime index 2 out of range"; the pin instantiates only what the
  entry reaches and prints `ok`.
  - The elaborator above MIR mints a `def`'s clone per discovered call and
    fails the program when one does not elaborate.
  - A struct method's clone already defers the same failure to
    `native::mono`, which reports it only where reached: the clone's body
    becomes `instantiation_failure_stub`'s `_mojito_instantiation_failed`
    call (`comptime/specialize.rs`, `mono/failure.rs`). The `def` path can
    reuse it.
  - Pinned by `conformance/probes/keyed_def_clone_fails_unreached.mojo`.
  - Found while landing R403 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R459 A reached struct instance instantiates every constructor
  overload, called or not**

  Problem: `S[5](3)` calls only `__init__(out self, y: Int)`, yet the
  sibling `__init__(out self)`, whose `[1, 2][Self.n]` is out of range at
  `n = 5`, fails the program with "keeps the parameter constant"; the pin
  instantiates only the called overload and prints `8`.
  - `Specializer`'s struct-instance declaration (`mono/specializer.rs`)
    enqueues every `__init__` overload the instance does not disprove, beside
    the copy, move, and destroy members lowering composes by name.
  - A constructor should be instantiated only by a call that reaches it, as
    an ordinary method already is.
  - Pinned by `conformance/probes/constructor_overload_uncalled_instance.mojo`.
  - Found while landing R403 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R70 A value-keyed `def` cannot forward its value to a keyed `def`**

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

- [ ] **R71 A compile-time-keyed `def` cannot be passed as a function value**

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

- [ ] **R72 A member-led arithmetic type argument does not parse in an alias
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

- [ ] **R73 An arithmetic `where` operand compiles for `def`s and struct methods
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

- [ ] **R74 `comptime assert` is not parsed**

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

- [ ] **R75 `reflect[T]` over a function's type parameter is rejected**

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

- [ ] **R76 `reflect[T]` of a non-struct type is rejected**

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

- [ ] **R269 `reflect[Self]` in a non-generic struct's method is rejected**

  Problem: `comptime r = reflect[Self]` then `r.field_count()` in a method of
  a plain struct prints the field count at the pin, while Mojito reports
  "not a compile-time value: unsupported compile-time type argument".
  - The elaborator walks a plain struct's method bodies with no binding for
    `Self`, so `reflect[Self]` has no subject.
  - Both the bound form and `comptime n = reflect[Self].field_count()` fail.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R278 `reflect[Self]` in a generic struct's method is rejected**

  Problem: `reflect[Self].field_count()` in a method of `struct Box[T]`,
  written there or inherited from a trait default under `comptime if
  reflect[Self].is_struct():`, prints the field count at the pin, while
  Mojito reports "not a compile-time value: unsupported compile-time type
  argument".
  - The elaborator's `type_from_anno` has no binding for `Type::SelfType`,
    so an instance clone cannot name its own type as a reflection subject.
  - The upstream reflective defaults (`Hashable.__hash__`,
    `Equatable.__eq__`) are written over `reflect[Self]`; inherited
    defaults are now struct methods, so this and R269 are what they wait on.
  - Probe: `conformance/probes/reflect_self_generic_struct_method.mojo`.
  - Depends on R269.
  - Model: Opus, Planned.

- [ ] **R279 A struct's associated value read as `Self.N` is rejected**

  Problem: `return Self.N` over `comptime N: Int = 3` in a struct's own
  method, or in a trait default it inherits, prints the value at the pin,
  while Mojito reports "'Self.N' is not a type parameter of the enclosing
  struct".
  - A bound parameter reads the member (`T.size` over `T: Fixed`), so only
    the struct's own spelling is missing.
  - Probe: `conformance/probes/associated_value_self_read.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R513 A `comptime for` over another struct's compile-time list is
  rejected**

  Problem: `comptime for v in K.L`, where `struct K` declares `comptime L =
  [1, 2, 3]`, sums the list at the pin, while Mojito reports "Undefined
  variable 'K'".
  - It fails the same way in a plain function and in a generic struct's
    method, so it is the loop header's reading of a type's member, not a
    template gap.
  - `Self.L` over the struct's own list is R279's spelling.
  - Probe: `conformance/probes/comptime_for_other_struct_list.mojo`.
  - Found while landing R8 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R500 A trait's compile-time value member read through a type
  parameter in a compile-time position is rejected**

  Problem: `comptime if T.K > 2:` or `SIMD[DType.int32, T.K - 1]` in `def
  g[T: HasK]()`, where `HasK` declares `comptime K: Int`, fails with "not a
  compile-time Int constant: unsupported associated comptime member
  access", where the pin folds `K` per instance.
  - `comptime for i in range(T.K)` in a method `m[T: HasK]` fails where a
    call reaches it ("function instantiation of `S.m` failed"). It ran
    before R310, on a per-call clone that folded `T`; a generic `def` with
    the same loop still clones and runs.
  - No parameter expression names an associated value of a type binder,
    and `native::mono` has no table of struct associated values to answer
    one.
  - Probe: `conformance/probes/trait_value_member_comptime_position.mojo`.
  - Found while landing R310 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R505 A trait's compile-time value requirement cannot declare a
  default**

  Problem: `comptime K: Int = 3` in a trait body fails to parse ("Expected
  newline, ';', or EOF at the end of statement"), where the pin accepts it
  and a conformer that omits `K` reads `3` through a bound (`T.K`).
  - The parser's trait `comptime` member takes no initializer; a struct's
    annotated member (`comptime N: Int = 3`) parses.
  - Conformance would then take the default for a witness that omits the
    member, and `ConformanceFacts::associated_values` would carry it so the
    elaborator answers `T.K` from it.
  - Found while landing R484 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R280 `comptime if conforms_to(T, X):` over a `def`'s parameter fails**

  Problem: `def tag[T: Movable](x: T)` holding `comptime if conforms_to(T,
  Copyable):` prints `copyable` at the pin, while Mojito stops with
  "invalid checked program: fn '$comptime$tag$0': register r0 has no
  checked type".
  - The template-served `comptime if` outlines its condition into a thunk,
    and the thunk's MIR carries `conforms_to` as an untyped call over type
    operands.
  - The same condition over `Self.T` in a struct method runs.
  - Probe: `conformance/probes/conforms_to_condition_generic_def.mojo`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R270 A module-scope reflection handle is rejected**

  Problem: `comptime R = reflect[Point]` at module scope, read in `main` as
  `R.field_count()` or through `comptime n = R.field_count()`, prints 2 at
  the pin, while Mojito reports "unknown type 'reflect'".
  - The same binding inside a function works.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R271 A `field_types()` result in a runtime position is rejected**

  Problem: `var t = reflect[Point].field_types()` compiles at the pin, while
  Mojito reports "type-valued or symbolic comptime values cannot materialize
  at runtime".
  - The crossing pass folds a runtime reflection query to its value's
    literal form, and a list of types has none.
  - Upstream's result is a `TypeList` value, which Mojito's checker types
    only in compile-time positions.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R77 A field's value cannot be read by reflection**

  Problem: `reflect[T].field_ref[i](x)` — the value of field `i` of `x` — is
  unsupported; the upstream hashing, equality, and writing defaults are
  written with it.
  - Mojito's defaults are Rust AST synthesis (`comptime/synth.rs`) and a
    lowering (`mojito-pliron/src/lower/print.rs`), so no bundled body needs
    it yet.
  - Under validation the result's type is the opaque field type
    (`types[i]`), which the arm-licensing rule already covers.
  - Depends on R74 for the upstream spelling of the defaults, and on R51,
    the body shape they are written in.
  - Model: Fable, Planned.

- [ ] **R78 A local compile-time type list is unknown in an instance's annotation**

  Problem: `var v: types[i] = ...` over `comptime types =
  reflect[T].field_types()` validates, but the instance reports "unknown
  type 'types'".
  - The elaborator resolves `f.T` over a bound handle (`resolve_reflected_type`,
    `comptime/eval.rs`) and nothing else in an annotation; `comptime FT =
    types[i]` then `var v: FT` is the working spelling.
  - Depends on R40 when the annotation sits in a `comptime for` body.
  - Model: Opus, Not Planned.

- [ ] **R79 A handle chain in a call's type argument is read as a value**

  Problem: `_unqualified_type_name[reflect[T].field_at[i].T]()` reports
  "expected a type, found a value" at the instance.
  - The elaborator's `resolve_reflected_param_arg` rewrites a `ParamArg::Type`
    only; the chain arrives as a value argument.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R82 A struct keyed on a `DType` named like a bundled binder is
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

- [ ] **R83 A parametric nested `def` named as a value reports its marker**

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
  - Depends on R71, which decides which clone a bare reference to a
    parametric `def` names; the message follows from that.
  - Model: Opus, Not Planned.

- [ ] **R85 A method forwarding its own `DType` parameter to a sibling
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

- [ ] **R90 A compile-time-keyed `def` applied to a loan-carrying type is
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

- [ ] **R98 A `UInt`-annotated `comptime` literal reads as an `Int`**

  Problem: `comptime U: UInt = 3` then `var u = U; u -= 4` prints `-1`
  where the pin prints `18446744073709551615`.
  - The elaborator inlines a module constant's value at every use, and an
    integer value materializes as a bare literal, so each use types as
    `Int` whatever the annotation says.
  - Sized-scalar, vector, and `Float64` annotations already bind at their
    declared type: their value folds to a typed SIMD constant
    (`comptime/elab.rs`, `StmtKind::Comptime`).
  - The fix is a `UInt` constant that materializes as `UInt(3)` at a value
    use without changing how a `UInt` value parameter is spelled.
  - Found while closing the annotated-constant task (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R485 An `Int`-annotated local `comptime` accepts a `Bool` value**

  Problem: `comptime m: Int = 1 > 0` in a function prints `True`, where the
  pin rejects it with "cannot implicitly convert 'Bool' value to 'Int'".
  - The checker's `StmtKind::Comptime` arm (`checker/statements.rs`) drops
    an `Int` annotation before typing the value, so the binding keeps the
    value's own type.
  - A template body's binding over its binders checks its annotation
    (`bind_template_comptime`) and is rejected as at the pin.
  - Same annotation-dropping family as R98.
  - Probe: `conformance/probes/comptime_int_annotation_bool_value.mojo`.
  - Found while landing R312 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R99 A method call whose compile-time argument names the caller's
  value parameter fails MIR verification**

  Problem: `s.scale[k](5)` inside `def run_at[k: Int](s: Doubler)`, beside
  `def scale[n: Int](self, value: Int)`, stops with "register r6 has no
  checked type", where the pin runs it.
  - A free function called the same way (`twice[k](5)`) runs, and so does
    the method call with a literal argument (`s.scale[2](5)`).
  - The same failure meets a call through a bound (`T: Scaler`), so a
    requirement default reading the method's parameter
    (`checker/bound_defaults.rs`) cannot yet be shown spelled from a
    caller's parameter.
  - Found while accepting requirement defaults over a method's parameters
    (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R100 A generic nested `def` with a by-value capture does not
  compile natively**

  Problem: `def inner[k: Int]() {var x} -> Int` runs on the VM and prints
  what the pin prints, but natively it is refused: "generic retained
  callable `outer$inner` captures by value".
  - Native monomorphization turns each call into a direct call to the
    instance, passing a by-reference environment's places as the lifted
    body's leading arguments (`capture_arguments` in
    `native/mono/specializer.rs`).
  - A `var` or moved capture is a snapshot held only by the closure value,
    which the direct call no longer reads; the snapshot needs a home the
    call can reach, such as a hidden local copied at the declaration.
  - `conformance/probes/generic_nested_def_var_capture.mojo` pins it.
  - Found while compiling capturing generic nested `def`s natively
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R498 A nested `def` keyed on a type pack fails MIR verification**

  Problem: `def inner[*Ts: Writable](*args: *Ts)` declared inside `def
  outer()` and called there (`inner(1, "a")`) stops with "callable contract
  names parameter `*Ts` of `inner` that no enclosing declaration binds",
  where the pin runs it.
  - The same `def` at module level is served by its template and runs.
  - Found while landing R253 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R106 A variadic struct's pack bound is not enforced at an explicit
  application**

  Problem: `Row[Plain, Int]()` over `struct Row[*Ts: Movable & Defaultable]`
  compiles and runs though `Plain` is not `Defaultable`.
  - The pin rejects it: "'Row' parameter 'Ts' has 'Defaultable & Movable'
    type, but value has type 'AnyStruct[Plain]'".
  - A body that relies on the bound then runs a member no element supports:
    `self.storage[i] = Self.Ts[i]()` constructs a `List[Int]` element, which
    the bundled `List` does not declare `Defaultable`.
  - A plain generic struct's bound is enforced (`Box[T: Defaultable]`).
  - Found while `Tuple`'s default initializer moved into source
    (2026-09-29).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R107 User code may spell the compiler-private `__RuntimeTuple`**

  Problem: `var storage: __RuntimeTuple[*Self.Ts]` in a user struct, and
  `__RuntimeTuple(*args^)` in its initializer, compile and run.
  - The pin has no such name, so the acceptance is Mojito's own.
  - A subscript of `__UninitStorage` is already refused outside the bundled
    library ("is compiler-private storage; use MaybeUninit from
    std.memory").
  - The lever is the same test on the declaring module, at the annotation
    and at the construction.
  - No fixture pins the acceptance.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R110 A nested `def` cannot read an enclosing function's
  `comptime` binding**

  Problem: `comptime n = 3` in `main`, read by a nested `def inner() ->
  Int: return n + 1`, is rejected ("Could not infer capture convention of
  the captured value n"), where the pin prints `4`.
  - A `comptime` float binding is rejected the same way.
  - The binding is a compile-time value, as an enclosing function's value
    parameter is, and a nested def already reads one of those without
    naming it (`assets/ok/nested_def_reads_enclosing_parameter.mojo`).
  - Probe: `conformance/probes/nested_def_reads_local_comptime.mojo`.
  - Found while a local `comptime` float was made to read as `Float64`
    (2026-09-30).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R130 A struct member's `where` clause over `TypeList[Self.Ts.values]()`
  is rejected**

  Problem: `def has_int(self) -> Bool where
  TypeList[Self.Ts.values]().contains[Int]()` on `struct Row[*Ts: AnyType]`
  runs at the pin and stops in Mojito with "TypeList[...] takes a pack
  projection ('Ts.values')".
  - The parser reads the bracket argument in type position, and
    `typelist_receiver` (`checker/constraints.rs`) accepts only the value
    spelling `Ts.values`.
  - Accepting the type spelling there compiles the clause, but the call then
    judges `.all[IsTriviallyCopyable]()` false on `Row[Int, Bool]`, so the
    pack's binding at a method call needs a look too.
  - `conforms_to(Self.Ts.values, Copyable)` in the same position runs.
  - The elaborator decides `.all`, `.any`, `.contains`, and `.length` over a
    bound pack. An `.all` or `.any` through a predicate alias stays
    undecided there, because the alias's body does not reach MIR.
  - Probe: `conformance/probes/typelist_self_pack_member_clause.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R131 A compile-time evaluation cannot call a generic struct's method
  that holds a `comptime if`**

  Problem: `comptime CAP = np(5)`, where `np` calls `Cell[Int](n).m()` and
  `m` holds a `comptime if`, prints at the pin and stops in Mojito with
  "Cell.m: unspecialized type-keyed method".
  - The evaluation's subprogram carries `m` as its template stub and mints
    no per-instantiation clone for it, as the production elaboration does.
  - The same struct beside an evaluation that does not call `m` runs.
  - Entry R7 moves the evaluation onto the elaborator's worklist by the
    request path of `docs/notes/ctfe-request-path.md`, which serves the
    instance and closes this.
  - Probe: `conformance/probes/ctfe_calls_comptime_if_struct_method.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R486 A compile-time evaluation cannot write a `Tuple` or a `t"…"`
  literal**

  Problem: `comptime s = label(3)`, where `label` returns `String(t"n={n}")`
  or `String((n, 2))`, prints at the pin and stops in Mojito with
  "TString.write_to: unspecialized type-keyed method" (or `Tuple.write_to`).
  - The evaluation's subprogram carries a variadic struct's `comptime for`
    member as its template stub and runs it erased, as R131's does.
  - A t-string there ran until 2026-10-07 only through MIR's eager
    concatenation fallback, deleted when the checker began building the
    `__make_tstring` call.
  - Entry R7 serves the instance from the worklist and closes this.
  - Probe: `conformance/probes/ctfe_writes_tuple_and_tstring.mojo`.
  - Depends on R7.
  - Model: Opus, Not Planned.

- [ ] **R132 A compile-time evaluation cannot call a value-keyed generic
  `def`**

  Problem: `comptime R = rep[3]()`, where `rep[n: Int]` recurses under a
  `comptime if n == 0`, prints `3` at the pin and stops in Mojito with
  "unknown compile-time function 'rep'".
  - The evaluation's subprogram excludes every compile-time-keyed `def`
    (`is_specializable_declaration_in`), so the call finds no body.
  - The same recursion under a runtime `if` runs in Mojito, on the erased
    body, where the pin expands without end; that is divergence R162
    (`ctfe-plain-keyed-recursion`).
  - Entry R7 closes this: a demand serves `rep[3]`, `rep[2]`, `rep[1]`,
    `rep[0]` from the worklist, each with its arm decided.
  - Probe: `conformance/probes/ctfe_keyed_recursion.mojo`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R134 `type_of(x)` is not accepted as a type**

  Problem: `var y: type_of(x) = 3` stops with a parse error at the `(`, and
  `unsafe_alloc[type_of(px)](1)` with "expected a type, found a value"; the
  pin runs both.
  - Spelling the type out (`unsafe_alloc[Pointer[Int, origin_of(x)]]`)
    runs.
  - Found while fixing a pointer dereference through a generic struct's
    field (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R135 A `comptime` constant built by a sized scalar constructor is
  rejected**

  Problem: `comptime K = Int32(4)`, at module scope or in a function, stops
  with "'Int32' is not a compile-time-callable function"; the pin binds it
  and prints `4`.
  - The elaborator folds a scalar construction only for `Int`, `UInt`,
    `Float64`, and `Bool` (`scalar_type_name` in `comptime/eval.rs`); any
    other call falls through to CTFE of a user function by that name.
  - `Int64`, `UInt8`, `Float32`, and the other `Scalar[dt]` aliases reject
    the same way.
  - Found while probing a scalar splat into a vector default (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R137 Checking a deeply nested generic struct type takes time
  exponential in its depth**

  Problem: `var a16 = W[W[…W[Int]…]](a15.copy())`, sixteen levels of
  `struct W[T: Copyable & Deinitable]` built one per line, takes minutes in
  `mojito check`, and every two more levels roughly triple the time.
  - Twelve levels take about 1.7 s beyond the check's fixed cost.
  - `wrap(wrap(…wrap(7)…))` over `def wrap[T](x: T) -> W[T]` grows the same
    way, about doubling per level from twelve on.
  - Declaring the same type without constructing it costs nothing, and
    nested calls of a generic `def` returning `T` stay flat.
  - A likely cause is a conformance of each level re-derived once per bound
    of the level below, with no memo.
  - Found while checking nested instance names (2026-10-03).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R138 An applied module constant is not a struct's own type
  argument**

  Problem: `comptime M = f(7)` shapes a vector width as the application
  (`SIMD[DType.float32, M]` is `SIMD[DType.float32, f(7)]`), but a struct
  argument over it is not that type.
  - `Buf[Int, M]` is rejected ("cannot be a struct argument yet"): a struct
    instance below the waist is named by a constant argument, and a
    residual value argument names no instance.
  - `Buf[M]`, a single bracket argument, parses as a subscript, and the
    elaborator folds `M` to `8` in it, so Mojito accepts `Buf[8]` where the
    pin rejects it against `Buf[f(Int(7))]`.
  - The lever is the request path: an instance keyed by an evaluated
    application names it by the evaluation, as the pin's `(FuncOp,
    operands)` key does.
  - Found while landing the register types over parameter expressions
    (2026-10-03). The request path landed with the `comptime if` entry the
    same day (`Specializer::demand_application`).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R272 A reflected field name materializes as `String`, not
  `StringSpan[ImmStaticOrigin]`**

  Problem: `materialize[names[i]]()` over `comptime names =
  reflect[T].field_names()` is a `String` in Mojito and a
  `StringSpan[ImmStaticOrigin]` at the pin, so the crossing diagnostic
  spells `Array[String, Int(2)]` where the pin spells
  `Array[StringSpan[ImmStaticOrigin], Int(2)]`.
  - The elaborator answers `field_names()` with a list of plain compile-time
    strings (`comptime/eval.rs::eval_reflection_method`), which materialize
    as `String` literals.
  - A program that annotates the element as a `StringSpan` is rejected where
    the pin runs it.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R497 A reflected field-type list materialized whole is rejected**

  Problem: `var all = materialize[types]()` over `comptime types =
  reflect[T].field_types()` prints the field count through `len(all)` at
  the pin and is rejected in Mojito with "type-valued or symbolic comptime
  values cannot materialize at runtime".
  - The pin's `field_types()` is a zero-sized `TypeList`, so the runtime
    value carries only its length.
  - A closed subject is rejected too, so this is no template-form gap.
  - Probe: `conformance/probes/reflected_field_types_materialize.mojo`.
  - Found while landing R365 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R303 A capturing generic callable parameter cannot be applied at
  its own compile-time arguments**

  Problem: `callback[2](40)` in `def invoke_captured[origins: OriginSet, //,
  callback: def[n: Int](Int) capturing[origins] -> Int]()`, bound to an
  `@parameter` closure that captures a local, runs erased and stops in the
  elaborator with "place keeps symbolic type `def[n: Int](Int)
  capturing[...] -> Int`".
  - The elaborator resolves an indirect call through a capture-free generic
    callable, or a capturing non-generic one passed as a trailing runtime
    argument, but never specializes a capturing generic closure at the
    call's compile-time arguments while passing its environment.
  - `conformance/fixtures/generic_anonymous_callables.mojo` (artifact
    conformance case `generic-anonymous-callables`) and
    `generic_callable_contract_defaults_override_implementation_defaults`
    (`tests/param_callable_value_test.rs`) pin it.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R327 An out-of-range value-pack index fails as an unsupported
  constant**

  Problem: `values[2]` in `def third[*values: Int]()` called as
  `third[1, 2]()` stops with "keeps the parameter constant `values[2]` in
  elaborated MIR", where the pin rejects the instantiation at compile time.
  - The verdict is right; the words are internal.
  - `Specializer::param_constant` (`mono/specializer.rs`) drops the
    out-of-range error `eval_ct` reports as `Unsupported`, so the verifier
    reports the surviving constant instead of the instantiation.
  - Found while landing R318 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R460 An out-of-range display index over a binder fails as an
  unsupported constant**

  Problem: `comptime q = [1, 2][n]` in `def f[n: Int]()` called as `f[5]()`
  stops with "keeps the parameter constant `[1, 2][n]` in elaborated MIR",
  where the pin rejects the instantiation at compile time.
  - The verdict is right; the words are internal.
  - The same cause as R327: `Specializer::param_constant`
    (`mono/specializer.rs`) drops the out-of-range error, so the verifier
    reports the surviving constant instead of a failed instantiation.
  - Found while landing R403 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R499 An invalid SIMD width computed per instance is reported as
  unverifiable MIR**

  Problem: `comptime w = twice(i + 1)` then `SIMD[DType.int32, w](1)` in a
  served `comptime for` stops with "specialized MIR that does not verify:
  ... SIMD width 6 is not a positive power of two" when a copy computes 6.
  - The pin reports "SIMD vector length must be a power of two between 1
    and 2^15" at the use.
  - The same width also read under a `comptime if` over `comptime b =
    flag(i)` reports "keeps symbolic type `SIMD[DType.int32, twice(i + 1)]`
    in elaborated MIR" instead.
  - A clone used to report "SIMD width must be a positive power of two, got
    6".
  - Both compilers reject the program; only the words differ.
  - Found while landing R253 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R340 A `comptime for` header cannot call a static method on `Self`**

  Problem: `comptime for i in range(Self.__len__())` in a variadic struct's
  method reports `Undefined variable 'Self'`, where the pin accepts it and
  upstream's `Tuple` spells every element loop that way.
  - Mojito's `tuple.mojo` keeps `range(len(Self.Ts))`.
  - The header is evaluated by the elaborator, which binds no `Self` for a
    static call.
  - Found while landing the `Tuple` default initializer (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R345 A list computed from packs still open answers only its length
  and its elements**

  Problem: `Self.Ts.reverse().all_conforms_to[Copyable]()` in a variadic
  struct's method stops with "TypeList.all_conforms_to over the computed
  list … whose packs are still parameters".
  - `all`, `any`, and `contains` over such a list are rejected the same way.
  - Over a closed list (`TypeList.of[…]().reverse()`) each of them folds.
  - Upstream's `slice`, `splat`, `map`, `filter_idx`, and `reduce` have no
    spelling in Mojito at all.
  - The lever is a pack query over a list expression rather than over a
    pack binder (`ParamKind::PackQuery`).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R348 A spread of a computed list binds only `Tuple`**

  Problem: `Holder[*Self.Ts.reverse()]()` over a user variadic struct stops
  with "call spread outside a specialized type pack", where the pin prints
  `3` for the flipped holder's length.
  - `Tuple[*Self.Ts.reverse()]` and
    `Tuple[*TypeList._concat[A.values, B.values]()]` resolve.
  - A `Variant[*Ts.reverse()]` is unspelled the same way.
  - Probe: a `Holder[*Ts: Movable]` whose method returns
    `Holder[*Self.Ts.reverse()]()`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R415 A static call spreading a variadic struct's collector in its
  own method is rejected**

  Problem: inside `struct V[*Ts: Writable]`, a method that forwards its
  `*b: *Self.Ts` collector to a static method (`Self.count(*b)`,
  `V[*Self.Ts].count(*b)`) fails with "no symbolic rule for an unbound
  pack", where the pin runs it.
  - The instance-method form (`self.each(*b)`) runs.
  - A bare `Self.` static on a parametric struct infers the struct's
    parameters from the arguments (`infer_struct_static_method`) instead of
    binding the enclosing `Self`, and a forwarded pack infers nothing.
  - Found while landing R256 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R436 A struct's `comptime` value member read through the type name
  is rejected**

  Problem: `print(E.k)` over `struct E: comptime k = 3` prints `3` at the
  pin, while Mojito reports "Undefined variable 'E'", and `G(E.k)` reports
  that no constructor overload matches.
  - R279 is the same member read as `Self.k` inside the struct.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R438 A `ref` binding of a pack element inside a `comptime for` fails
  elaboration**

  Problem: `comptime for i in range(Ts.length): if i == idx: ref arg =
  args[i]; arg.write_to(w)` runs at the pin, while Mojito stops with "slot
  `arg` typed over a comptime for index is used outside its loop".
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Behavioral Divergences From The Pinned Mojo *(recurring — reopens at every nightly re-pin)*

Track: `divergences`.

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

Behavioral divergences from the pinned Mojo, to burn to zero. Every new
divergence lands here with a probe or a `cases.tsv` `mojito-only` /
`output-diff` row, and leaves when its probe promotes to an `assets/ok`
fixture. Each entry's `Ledger name` is the slug `conformance/cases.tsv` uses
for it, and an entry that waits on another names it. Five divergences are
retained on purpose and re-probed rather than fixed; they are listed in
[`docs/non-goals.md`](non-goals.md).

- [ ] **R525 Materialization errors omit Mojo's contextual notes**

  The crossing diagnostic has one message where Mojo distinguishes a bare read from a call argument.
  - A bare read suggests `materialize`; a call argument suggests evaluating the whole call with `comptime`.
  - `TypeError` has no diagnostic-note channel.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R162 A compile-time evaluation of a keyed function that recurses
  under a runtime `if` prints a value where the pin expands it without
  end**

  Problem: `comptime R = rep[3]()`, where `rep[n: Int]` calls `rep[n - 1]()`
  under a runtime `if n == 0`, prints `6` in Mojito. The pin expands the
  instantiation without end (killed after 60 s, its depth unlimited by
  default).
  - Cause: Mojito's compile-time evaluation runs the erased body with `n`
    reified at run time.
  - Pinned by `conformance/probes/ctfe_plain_keyed_recursion.mojo`.
  - Closes with R7, when the evaluation demands concrete instances and the
    instance budget stops the expansion.
  - Ledger name: `ctfe-plain-keyed-recursion`.
  - The work is R7's, so this entry is taken as-is when R7 lands.
  - Depends on R7, which makes the evaluation demand concrete instances.
  - Model: Opus, Not Planned.

- [ ] **R163 `len` over a runtime pack is accepted in a `comptime for`
  header the pin rejects**

  Problem: `comptime for i in range(len(items))` over a runtime pack
  `*items: *Ts` runs in Mojito and is rejected upstream ("cannot use a
  dynamic value in call argument"). Both accept `items.__len__()` there.
  - Pinned by `conformance/probes/pack_len_comptime_for_header.mojo`.
  - The lever is the builtin `len` over a `VariadicPack` under source
    validation and the elaborator's `len(args)` fold
    (`specialize.rs:generate_def_spec`).
  - Both answer the arity where the pin treats the call as dynamic.
  - Ledger name: `len-over-pack-in-comptime-for-header`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R442 `TString` is in scope in user code, where the pin has no such
  name**

  Problem: `TString(*args^)` in a user `def` runs in Mojito, where the pin
  rejects it with "use of unknown declaration 'TString'".
  - The bundled `TString` struct is visible as a prelude name, though the pin
    keeps its t-string type out of the user's namespace.
  - No t-string needs the name: the checker calls `__make_tstring` by
    module path, so the prelude export only serves user spellings.
  - Found while landing R316 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R164 A trait method with a trailing `where` clause is accepted, though
  the pin does not support `where` on trait methods**

  Problem: a trait method declared with a trailing `where` clause (`def
  push[H: Movable](mut self, value: H) where conforms_to(H, Copyable): ...`)
  is accepted by Mojito and rejected upstream ("'where' clauses on trait
  methods are not supported", 2026-09-26).
  - A struct's witness of such a requirement is rejected as not matching
    the signature, with or without the same clause.
  - Pinned by `conformance/probes/where_on_trait_method.mojo`.
  - The lever is the trait-method arm of `check_trait`
    (`checker/traits.rs`), which compiles the clause onto the method's last
    binder.
  - Ledger name: `where-on-trait-method`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R165 A free function returning an owned interior of its argument
  escapes the call-result aliasing rule, so `w = keep(view_x(w))` runs**

  Problem: a free function whose return declares an owned interior of an
  argument is not judged by the call-result aliasing rule
  (`checker/origins/result_alias.rs`). So `w = keep(view_x(w))` runs in
  Mojito and is rejected upstream.
  - `view_x(v: W) ->
    StringSpan[origin_of(v.x)._get_owned_interior["bytes"]]` carries its
    argument's origins unprojected.
  - Cause: only methods record `view_result_interiors`.
  - A free call needs the same side table keyed by the projected parameter.
  - Free-function signatures keep no source return type the call site can
    read, so the plan picks where the parameter projection is recorded.
  - Ledger name: `result-alias-rule-coverage`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R166 Unpack-assigning a call over a view of the assigned local is
  rejected as a conflict the pin does not see**

  Problem: `a, b = pair(a.rstrip())` runs upstream (`ab 1`), while Mojito
  rejects it with "access to 'a' conflicts with live reference
  '$arg_loan_r5'".
  - Cause: the argument's view anchor outlives the call into the unpacking
    store.
  - The anchor's statement-end keep-alive is what every other call argument
    relies on.
  - Shortening it for unpacking needs its fallout checked first.
  - Ledger name: `unpack-assign-call-over-viewed-local`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R167 Moving a `var` parameter into a local collection in a generic
  method is rejected as a use of an uninitialized value**

  Problem: in a generic struct's method, `result.append(value^)` into a
  local `List[Self.T]`, with `value` a `var` parameter of type `Self.T`,
  then `return result^`, runs upstream. Mojito rejects it with "use of
  uninitialized value 'value'".
  - Pinned by `conformance/probes/moved_parameter_into_local_list.mojo`.
  - A parameter whose type may carry loans stands for the caller's loans by
    its own place (`declarations.rs`, the parameter's aggregate origins).
  - So `append`'s transfer effect installs a loan on `value` in `result`,
    and the erased generic body reads `value` after it moved.
  - A field of `self` as the destination installs nothing in the frame
    (`install_call_transfers`).
  - A concrete element type carries no loans.
  - So only the erased body with a local destination rejects.
  - The lever is what a moved source's stand-in place means once the source
    is gone: its loans outlive the move, the place does not.
  - Ledger name: `moved-parameter-into-local-collection`.
  - Depends on R49, the same stand-in place through a sibling call's `mut`
    argument.
  - Model: Opus, Planned.

- [ ] **R168 `String`, `List` and four other bundled stdlib types are not
  trivially movable in Mojito, though they are upstream**

  Problem: `IsTriviallyMovable[String]`, `IsTriviallyMovable[List[Int]]`,
  and the `MaybeUninit` conformances that follow from them are `False` on
  Mojito and `True` upstream.
  - Cause: six bundled stdlib types (`String`, `List`, `Array`, `Dict`,
    `Set`, `Optional`) declare an explicit `__init__(out self, *, deinit
    move: Self)` where upstream relies on the implicit bitwise move.
  - The predicate itself agrees on hand-written structs.
  - Deleting the six move constructors is the fix.
  - It hands every heap-owning move to the compiler-generated path on both
    backends, so the plan checks that path first.
  - Ledger name: `trivially-movable-stdlib-types`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R169 A `ref` binding to a register-passable value is accepted,
  though the pin rejects it for having no memory origin**

  Problem: a `ref` binding to a register-passable value is rejected upstream
  (`value of type 'Int32' doesn't have a memory origin in 'ref' binding`)
  but accepted by Mojito.
  - It covers `ref y = f()` for an `Int`-returning `f`, `ref y = a + 1.0`,
    and a SIMD lane read `ref lane = v[i]`.
  - The lane binding also writes through to the vector, so `lane += 2`
    changes `v`.
  - A memory-backed temporary (`ref x = make_list()`) is accepted by both.
  - The lever is the `StmtKind::RefDecl` arm in
    `crates/mojito-checker/src/checker/statements.rs`, whose
    `materialized_reference_actual` fallback materializes any value.
  - Pinned by `conformance/probes/ref_binding_register_value.mojo`.
  - The lever is named, but the rejection reaches wide fixture fallout, so
    it wants its own pass with the fallout enumerated first.
  - Ledger name: `ref-binding-register-value`.
  - Depends on R54, which wants the same fallback to materialize a literal
    for a `ref` parameter: the two must agree on what it may materialize.
  - Model: Opus, Planned.

- [ ] **R170 Writing a `Tuple` element (`t[0] = 9`) is rejected, though the
  pin's subscript is a mutable place**

  Problem: `t[0] = 9` on a `Tuple` runs upstream and prints `9`, while
  Mojito rejects it with "invalid assignment target: Tuple elements are
  immutable". Upstream's `__getitem__[idx](ref self)` returns `ref [self]`,
  so the subscript is a mutable place.
  - The rejection is a checker rule that predates the reference-returning
    `__getitem_param__` the bundled `std/builtin/tuple.mojo` now declares.
  - So the declaration and the rule disagree about the same hook.
  - Withdrawing it also decides `std/collections/pack_tuple.mojo`, whose
    accessor returns a copied value precisely to keep the write rejected
    (`self_hosted_pack_tuple_preserves_tuple_restrictions`).
  - Pinned by `conformance/probes/tuple_element_write.mojo`.
  - The lever is one rule, but making tuple elements writable changes what
    every tuple place means to ownership analysis.
  - The plan enumerates that fallout first.
  - Ledger name: `tuple-element-write`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R171 Seven corpus fixtures compute different numbers from the pin at
  arithmetic edge cases such as an over-wide shift**

  Problem: seven corpus fixtures compute different numbers from the pin,
  found by the 2026-09-12 stdout sweep and listed in
  [`conformance/assets-mojo-output-diffs.tsv`](../conformance/assets-mojo-output-diffs.tsv)
  under `arithmetic`.
  - The clearest is a shift past the bit width
    (`assets/ok/pliron_straightline.mojo`: `a << 65`, where the pin prints
    7 and Mojito 51102306).
  - The SIMD shift and floor-division fixtures disagree wholesale.
  - Also here: an out-of-range float-to-int cast and `Float64` `**`.
  - `round()`'s half-way case left this list on 2026-09-13, when `round`
    became ties-to-even on both backends.
  - Each case needs the pin's rule established before Mojito's is changed.
  - `docs/native-abi.md` already defines some of them deliberately (wrapping
    overflow), so the plan decides which are bugs and which are recorded
    choices.
  - Ledger name: `native-arithmetic-edge-cases`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R172 Binding a capturing closure to a method's compile-time callable
  parameter retypes the captured value as `None`**

  Problem: a method whose compile-time callable parameter binds a capturing
  closure is rejected — "operator Mul is not defined for Int and None" —
  while the pin runs it. The same closure through a free function is
  `assets/ok/lambda_hof.mojo`.
  - The method call is what breaks it: the program runs once the
    `runner.apply[scale](5)` line goes.
  - So binding the closure to a *method's* parameter is what retypes the
    captured `factor` as `None`.
  - Pinned by `conformance/probes/method_capturing_callable_parameter.mojo`.
  - The native side of this shape is unbuilt behind the checker.
  - Only the direct-call arm promotes a capturing callable argument to a
    runtime parameter (`mono/promote.rs`).
  - So a `MethodCall` carrying one would still reject contextually at
    monomorphization.
  - Ledger name: `method-capturing-callable-parameter`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R173 Two corpus fixtures run their destructors later than the pin
  does**

  Problem: two corpus fixtures run the same destructors later than the pin
  does (`conformance/assets-mojo-output-diffs.tsv`, family `drop-timing`):
  `assets/ok/owned_pointer_api.mojo` and
  `assets/ok/try_region_drop_timing.mojo`.
  - The two `MaybeUninit` fixtures that used to skip destructors outright
    left this list on 2026-09-17, when owning temporaries gained their
    hidden slots.
  - Both remaining cases are orderings rather than omissions.
  - The plan establishes where the pin runs each destructor before Mojito's
    schedule is moved.
  - Ledger name: `destructor-timing-against-the-pin`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R174 A reference into what a `Pointer` field borrows cannot name the
  element sub-origin the pin requires**

  Problem: a method returning a reference into what a `Pointer[T, Self.o]`
  field borrows names the region with the struct's own origin parameter in
  Mojito. The pin distinguishes the element sub-origin
  (`origin_of(o["element"])`) and wants `ref[origin_of(self.src[][i])]`,
  which Mojito's escape check rejects.
  - Cause: no `Origin` variant carries a projected `SelfParam`.
  - Two `assets/origin_ok` fixtures moved for it.
  - Five `Pointer` iteration twins
    (`assets/extensions/ok/pointer_field_reference_yielding_iteration*`,
    `pointer_field_comprehension_borrowed_named_source`,
    `pointer_field_parametric_mut_iterator_read`) joined their `ref`-field
    originals.
  - The fix adds a projected receiver origin to
    `mojito_types::origin::Origin`, which MIR text, verification, and
    substitution all read.
  - Ledger name: `pointee-element-reference-return`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R175 A function-local `comptime` constant must be named in a nested
  `def`'s capture list, where the pin rejects naming it there**

  Problem: a function-local `comptime` constant read from a nested `def` is
  an ordinary immutable local in Mojito, so it must be named in the capture
  list. The pin treats it as a compile-time constant and rejects naming it
  there.
  - Mojito cannot simply stop requiring the capture: the constant has real
    storage in the outer frame and the lifted function has no binding for
    it.
  - The fix makes a folded `comptime` local a constant the lifted body can
    read.
  - That is a lowering change, not a scope-rule change.
  - Ledger name: `local-comptime-capture`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R176 Capturing callables are accepted as runtime arguments, typed
  locals and `mut self` conformers, where the pin takes them only as
  compile-time parameters**

  Problem: four callable-shape leniencies. Mojito passes a capturing lambda
  as a runtime argument, binds one to a local under an explicit
  `capturing[...]` annotation, and accepts a `mut self` `__call__` as a
  `def(...)` conformer.
  - The pin takes a capturing callable only as a compile-time parameter,
    and refuses a lambda even there.
  - The pin types a capturing lambda as a plain `def(...) -> T` that
    converts to nothing.
  - The pin wants a read receiver on `__call__`.
  - Three corpus fixtures moved to `assets/extensions/`.
  - Three more were respelled onto `thin` contracts and `@parameter def`
    arguments.
  - The four share one question — what a capturing callable *value* is —
    and the plan settles that before any rejection.
  - Ledger name: `capturing-lambda-argument`, `capturing-lambda-locals`,
    `owned-capture-closure-locals`, `mut-self-callable-struct`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R177 A struct field pointing into an owned interior of a generic
  origin parameter type-checks, though the pin calls that interior
  never-initialized**

  Problem: a struct field typed `Pointer[T,
  Self.origin._get_owned_interior["tag"]]` over a generic origin parameter
  is accepted by Mojito. The pin parses the projection but calls the
  interior reference never-initialized, so the carrier struct does not
  type-check there at all.
  - Two `assets/ownership_ok` fixtures moved for it.
  - Upstream's owned-interior origins are real; what differs is which
    structs may name one.
  - The plan probes that rule before Mojito narrows.
  - Ledger name: `interior-generation-view-consume`,
    `interior-generation-view-drop`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R178 Mojito's `for` yields the iterable's `Element` where the pin
  yields the iterator's `Iter.Element`**

  Problem: Mojito's `for` yields the *iterable*'s `Element`, so one
  associated type serves a generic signature and the loop. The pin yields
  the *iterator*'s `Iter.Element` and will not convert between the two
  without an identity clause Mojito does not implement.
  - `assets/extensions/ok/iterable_associated_element.mojo` has no `main`.
  - Its `for` over a trait bound that declares only `Element` never
    compiled upstream.
  - Upstream, even a real `Iterable` bound abandons its `AnyType` iterator
    temporary.
  - Ledger name: `iterable-element-identity`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R179 Mojito's stdlib ships algorithm modules and owning-family
  container APIs that upstream does not have**

  Problem: two stdlib surfaces upstream does not have — `std.algorithms`,
  `std.collections.string_dict`, and the owning-family container APIs
  (`deinit_with`, `clear_with`, displacement-returning `insert`).
  - Two corpus fixtures moved to `assets/extensions/` for them.
  - Whether these leave or stay is a stdlib-shape decision, not a
    respelling.
  - Ledger name: `mojito-only-stdlib-algorithms`,
    `owning-family-container-apis`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R180 A conditional partial move joined with a whole move is
  accepted, though one path leaves a hole the pin rejects**

  Problem: a conditional partial move on one branch joined with a whole move
  on the other is accepted. The first path reaches the exit with a hole the
  pin rejects (`field 'p.a' destroyed out of the middle of a value`).
  - Cause: the three-point move lattice joins `a: MaybeMoved` under an
    intact base with a wholly moved base into a state it cannot tell from
    intact-or-wholly-moved.
  - Pinned by `conformance/probes/partial_move_join_imprecision.mojo`.
  - The lever is a fourth lattice point, or a per-node "may hold a hole"
    flag that survives joins.
  - The plan picks one.
  - Ledger name: `partial-move-join-imprecision`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R181 Assigning a call over a `Span` of the assigned `List` is
  rejected as a conflict the pin does not see**

  Problem: `xs = rebuild(Span(xs))` runs upstream (`1`) and is rejected in
  Mojito with "access to 'xs' conflicts with live reference 'xs'".
  - The temporary argument's anchor now ends before the store, as for the
    passing `s = String(StringSpan(s))`.
  - But the assigned `List[Int]` result still records a loan on `xs`, so
    the store conflicts with the new value itself.
  - Pinned by
    `conformance/probes/assign_plain_span_argument_over_list.mojo`.
  - Where the result's loan comes from (MIR `aggregate_borrows` or a
    replayed transfer effect) is not yet known.
  - Ledger name: `assign-plain-span-argument-over-list`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R182 A view struct with a direct `ref` field still widens a
  returned field view to `origin_of(self)`, which the pin rejects**

  Problem: a view struct that stores its source in a direct `ref` field
  still widens a returned field view to a declared `origin_of(self)`.
  - `def view(ref self) -> View[origin_of(self)]` returning `View(source,
    0)` over `ref source = self.items` passes when `View` holds `ref[o]
    List[Int]`.
  - The `Pointer`-field twin rejects with upstream's "cannot implicitly
    convert 'View[origin_of(self.items)]' value to
    'View[origin_of(self)]'"
    (`assets/type_error/return_origin_widening_field_view.mojo`).
  - Why the tail escapes `reconcile_return_origin_tails`
    (`checker/origins/solve.rs`) is not yet known.
  - The likely lever is the tail the `ref`-field constructor binds.
  - Pinned by `assets/extensions/ok/ref_field_view_for_temporary.mojo`,
    `ref_field_view_ref_yield.mojo`, `ref_field_view_method_return.mojo`,
    `ref_field_drain_mut_method.mojo`, and
    `ref_field_chained_view_call.mojo`.
  - The rule already exists; the gap is one constructor path.
  - Ledger name: `ref-field-return-origin-widening`.
  - Depends on R27, which tightens the returned-origin comparison this
    constructor path escapes.
  - Model: Opus, Planned.

- [ ] **R183 `s[0]` on a `String` yields an `Int` where the pin yields a
  character, and two other texts differ from the pin**

  Problem: `s[0]` on a `String` yields a character upstream and an `Int` in
  Mojito (`assets/ok/nominal_string_indexing.mojo`, `h` against `104`).
  - Two smaller text divergences ride along in the same manifest under
    `one-off`.
  - The raised `DictKeyError` renders differently
    (`assets/ok/self_hosted_dict.mojo`).
  - Reflection prints `<unprintable>` upstream where Mojito prints the
    element types (`assets/ok/type_names_applied_elements.mojo`).
  - The subscript's element type is a declaration change in the stdlib's
    `String`.
  - The other two are texts to match.
  - Ledger name: `string-subscript-element`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R184 `Int / Int` is true division into `Float64` in Mojito, where
  the pin truncates back to `Int`**

  Problem: `Int / Int` is true division into `Float64` in Mojito. The pin
  truncates back to `Int` and divides only an `IntLiteral` pair into a
  float.
  - An `output-diff` row, not a rejection, so it is not on the burn-down.
  - The result type of one operator changes, and every fixture and stdlib
    body that divides integers moves with it.
  - The template-fact derivation materializes a folded name's `i / 2` to
    the template's `Float64` (`folded_arithmetic`).
  - Once the template records `Int` there it refuses.
  - But the elaborator's literal fold then divides an `IntLiteral` pair
    into a float where the pin keeps `Int`.
  - Ledger name: `int-true-division`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R185 Infix SIMD comparisons are elementwise at every width, where
  the pin limits `<` and friends to `Scalar` and gives `==` whole-vector
  meaning**

  Problem: Mojito's `<`/`<=`/`>`/`>=` are elementwise at every width and its
  `==`/`!=` compare lane by lane. The pin constrains the strict
  inequalities to `Scalar` and gives `==`/`!=` whole-vector meaning,
  pointing at `SIMD.lt(...)`.
  - Mojito has had `SIMD.lt(...)`, along with `le`/`gt`/`ge`/`eq`/`ne`,
    since 2026-09-13.
  - Six corpus fixtures were respelled onto the methods.
  - Withdrawing the infix spelling is a leniency to remove, with fallout
    across the stdlib's SIMD bodies.
  - Until then only a width-one SIMD conforms to `Equatable` and
    `Comparable`.
  - The pin's vectors conform too, with `==` reducing the lanes.
  - Ledger name: `simd-infix-comparison`.
  - Depends on R143, the scalar comparison methods: once the infix spelling
    goes, `x.ne(y)` is the only ordered comparison left on a scalar, and it
    does not resolve there yet.
  - Model: Fable, Planned.

- [ ] **R186 SIMD and `Int` constructors accept a narrowing element, an
  inferred width and a `FloatLiteral` the pin rejects**

  Problem: three small constructor leniencies.
  - Mojito narrows a SIMD element argument to the lane type, wrapping an
    out-of-range literal and a wider runtime value. The pin wants the
    lane's own scalar.
  - Mojito infers an unbound SIMD width from the argument count. The pin
    wants a written-out width.
  - Mojito truncates a `FloatLiteral` straight to `Int`. The pin wants the
    `Float64` it truncates from.
  - Three leniencies to withdraw in one pass.
  - Ledger name: `simd-element-narrowing`, `simd-inferred-width`,
    `float-literal-to-int`.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R187 `reduce_mul` over `Float32` lanes folds left at lane precision,
  where the pin reduces pairwise**

  Problem: `reduce_mul` over `Float32` lanes folds left at lane precision in
  Mojito and pairwise in the pin (`319256416.0` against `319256448.0` on
  sixteen lanes).
  - `assets/ok/simd_wide_widths.mojo` now reduces an exactly representable
    vector to avoid it.
  - The float-format entry (R159) hides the same rows.
  - The reduction shape changes in the VM and in the
    `llvm.vector.reduce.fmul` lowering together.
  - Ledger name: `float32-reduce-ordering`.
  - Depends on R159, whose float-format difference hides the same rows.
  - Model: Fable, Planned.

- [ ] **R188 `sp[byte=i]` and `s[codepoint=i]` yield a byte value and a
  `Codepoint`, where the pin yields one-byte and one-codepoint views**

  Problem: `sp[byte=i]` reads the byte value in Mojito and returns the
  one-byte view upstream. `s[codepoint=i]` is a `Codepoint` in Mojito and a
  one-codepoint `StringSpan` upstream.
  - The same shape as `string-subscript-element` (R183), on the keyword
    subscripts.
  - Declaration changes in the stdlib's `String`/`StringSpan`.
  - Ledger name: `string-span-byte-index`, `string-codepoint-index`.
  - Depends on R183, the same declaration change on the positional
    subscript.
  - Model: Fable, Planned.

- [ ] **R189 `String.split` returns owned strings where the pin returns
  views, so assigning over the split source runs only in Mojito**

  Problem: `String.split`/`splitlines` return `List[String]` in Mojito and
  owned-interior `StringSlice` views upstream. So `var parts = s.split("
  ")` then `s = String(parts[0])` runs in Mojito and hits upstream's
  call-result aliasing rejection.
  - Pinned by `conformance/probes/split_returns_owned_strings.mojo`.
  - An API shape change with display and iteration fallout across every
    `split` caller.
  - Ledger name: `split-returns-owned-strings`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R190 A contiguous `List` slice is an owned `List` where the pin
  returns a borrowing `Span`**

  Problem: a contiguous `List` slice is an owned `List` in Mojito and a
  borrowing `Span` upstream, so only Mojito returns one from a `->
  List[Int]` function.
  - The call-result aliasing rule rides on it: upstream rejects `xs =
    rebuild(xs[0:1])` because the slice views `xs`'s owned elements.
  - Mojito's copy borrows nothing and runs.
  - Pinned by `conformance/probes/list_slice_copies.mojo`.
  - The return type of `List.__getitem__(ContiguousSlice)` changes, and
    every caller that owns the result moves with it.
  - Ledger name: `contiguous-slice-result`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R191 `Int` conforms to `Floatable` and `len` resolves from a bare
  `__len__`, though the pin allows neither**

  Problem: Mojito conforms `Int` and an integer literal to `Floatable`.
  Upstream conforms neither, so a `Floatable`-bounded helper takes only a
  float there.
  - Mojito also resolves `len(x)` from a bare `__len__` where the pin wants
    a declared `Sized` conformance.
  - `assets/ok/dunder_index.mojo` and `assets/ok/self_hosted_vec.mojo` now
    declare it.
  - Two conformance leniencies to withdraw.
  - Ledger name: `int-is-floatable`.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R192 Mojito keeps split slice kinds, unknown struct decorators and
  the `ImplicitlyDeletable` alias, which the pin has dropped or never had**

  Problem: three spellings the pin has dropped or never had.
  - Mojito splits upstream's single `Slice` into
    `ContiguousSlice`/`StridedSlice` and overloads subscripts on the kind.
  - Mojito ignores an unmodeled struct decorator where the pin rejects an
    unknown one (`@value` is now unknown there).
  - Mojito still normalizes `ImplicitlyDeletable` to `Deinitable`, which
    the pin has removed.
  - Each is a name or a type to withdraw, with stdlib and fixture fallout.
  - Ledger name: `slice-descriptor-kinds`, `unmodeled-struct-decorator`,
    `implicitly-deletable-alias`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R193 Two live interior references from `ref self` accessors on one
  owner are accepted, though the pin invalidates the first**

  Problem: `print(t.value_at(0), t.value_at(1))`, two `ref self` accessors
  returning an element's interior on one `var` owner, runs in Mojito and is
  rejected upstream ("use of invalidated interior reference
  't.entries["element"]'").
  - Pinned by
    `conformance/probes/interior_reference_two_live_accessor_results.mojo`.
  - The pin takes the second call as a mutable borrow that invalidates the
    first result.
  - A `List` subscript pair (`l[0], l[1]`) passes both.
  - Mojito records no conflict between the two results.
  - The lever is where a `ref self` call's interior reference meets a later
    mutable borrow of the same owner.
  - Found while deriving element-field reference results; not root-caused.
  - Ledger name: `two-live-accessor-interior-references`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R195 A `Pointer` type argument spelling a local's uninitialized
  interior origin is accepted, though the pin rejects it as a use**

  Problem:
  `unsafe_alloc[Pointer[Int, origin_of(a)._get_owned_interior["element"]]](1)`
  over a local `Array` runs in Mojito and is rejected upstream ("use of a
  never-initialized interior reference 'a["element"]'").
  - Pinned by
    `conformance/probes/pointer_type_argument_uninitialized_interior.mojo`.
  - The pin treats the spelled interior origin as a use of the local's
    interior, which nothing has initialized.
  - Mojito resolves the spelling to a place origin and checks no
    initialization of the interior.
  - Found while baking an interior-projected `Pointer` type argument into
    its clone (2026-09-28).
  - Ledger name: `pointer-type-argument-uninitialized-interior`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R245 A compile-time `break` or `continue` in a `comptime for` the
  elaborator unrolls in the AST is rejected**

  Problem: the pin honors a `break` or `continue` in every `comptime for`;
  Mojito honors one only in a loop its template serves (a `range` loop of a
  generic `def`), and rejects it with "a 'break' or 'continue' in a comptime
  for over 'i' the elaborator unrolls" everywhere else.
  - A loop in a non-generic `def` or a method, over a compile-time
    collection or a pack, or with a local `comptime` binding in its body is
    unrolled above MIR, where a spliced copy's `break` would leave the
    enclosing loop instead (`comptime/elab.rs::unroll_comptime_for`).
  - Closes when every `comptime for` is served by its template (R246, R2)
    and a method's compile-time control flow is (stage P3e of
    `docs/parametric-mir-plan.md`).
  - Ledger name: `comptime-for-break-in-unrolled-loop`.
  - Depends on R246.
  - Model: Opus, Not Planned.

- [ ] **R273 A runtime read of a symbolic `field_names()` binding in a
  generic body that is never called is accepted**

  Problem: `print(names[i])` over `comptime names = reflect[T].field_names()`
  in a generic `def` that no call instantiates runs in Mojito, while the pin
  rejects the template ("cannot materialize comptime value of type
  'Array[StringSpan[ImmStaticOrigin], ...]'").
  - An instantiated body is rejected as the pin does: the elaborator's
    crossing pass sees the closed list per instance
    (`assets/type_error/comptime_field_names_runtime_use.mojo`).
  - Source validation types `names[i]` and `len(names)` through
    `checker/reflection.rs::infer_reflection` without knowing whether the
    position is a runtime one; a compile-time binding or `materialize`
    operand reaches the same path.
  - Pinned by `conformance/probes/reflected_names_runtime_read_uncalled.mojo`.
  - Ledger name: `reflected-names-runtime-read-uncalled`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R349 Two packs in one parameter list are accepted**

  Problem: `def joined_len[*As: Movable, *Bs: Movable](a: Tuple[*As], b:
  Tuple[*Bs])` runs in Mojito, where the pin stops with "cannot have two
  '*' markers in the same parameter list".
  - A method's own pack beside its struct's pack is legal at the pin.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R350 `deinit` on a parameter that is not of `Self` type is accepted**

  Problem: `def take(self, deinit other: Lin)` on a struct other than `Lin`
  is accepted in Mojito, where the pin stops with "'deinit' must only be
  applied to arguments of Self type".
  - `Tuple.concat`'s `deinit other: Tuple[*OtherTs]` is the legal shape:
    another instance of the same struct.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R356 A generic `def` returning a concatenation of its own pack with
  itself is accepted**

  Problem: `def doubled[*Ts: ImplicitlyCopyable](t: Tuple[*Ts]) ->
  Tuple[*TypeList._concat[Ts.values, Ts.values]()]: return t.concat(t)` runs
  in Mojito, where the pin stops with "cannot implicitly convert
  'Tuple[*#kgen.param_list.concat(Ts.values, Ts.values)]' value to" the same
  spelling.
  - Mojito closes both lists to one canonical concatenation, so the result
    and the annotation are one type.
  - `flip`'s `Tuple[*Ts.reverse()]` result is accepted by both.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R404 A raising call in a compile-time collection literal is
  accepted**

  Problem: `comptime for x in [twice(n), n]:` over a `def twice(n: Int)
  raises -> Int` runs in Mojito, in a generic `def` or a method, where the
  pin stops with "cannot call raising function in collection literal".
  - A runtime list literal (`var l = [twice(1), 2]` in a raising body) is
    accepted by both.
  - Found while landing R400 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R408 A `Variant[*Self.Ts]` field is accepted over a pack not
  bounded `Deinitable`**

  Problem: `struct Box[*Ts: Copyable]` holding `var v: Variant[*Self.Ts]`
  compiles and runs in Mojito, where the pin rejects the field: "field 'v'
  has non-'Deinitable' type 'Variant[*Ts.values]'".
  - `Variant` is `Deinitable` only where every alternative is, and the pin
    demands that of every field at the declaration.
  - Pinned by `conformance/probes/variant_pack_field_not_deinitable.mojo`.
  - Ledger name: `variant-pack-field-not-deinitable`.
  - Found while landing R4 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R423 A writer overriding `write` misses the nested `write` a
  scalar's `write_to` makes at the pin**

  Problem: a `Writer` whose own `write` wraps each argument
  (`self.text += "<"`, then `args[i].write_to(self)`) prints `<1x>` for
  `b.write(1, "x")` in Mojito and `<<>1x>` at the pin.
  - The pin's `Int.write_to` and `Float64.write_to` are Mojo bodies that
    call `writer.write` for their digits, which re-enters the override.
  - Mojito formats a builtin scalar in the host and hands the text to
    `write_string` (the `$write_formatted` primitive), so the override is
    never re-entered.
  - Pinned by `conformance/probes/writer_override_scalar_write_to.mojo`.
  - Ledger name: `writer-override-scalar-write-to`.
  - Found while landing R414 (2026-10-06).
  - Depends on R276, which moves the scalar text into Mojo bodies.
  - Model: Opus, Not Planned.

- [ ] **R425 A malformed literal `format` template aborts at run time where
  the pin rejects it at compile time**

  Problem: `"[{:>5}]".format(3)` and `"{} {}".format(1)` fail to compile at
  the pin, while Mojito compiles them and aborts when the call runs.
  - Upstream's `StringLiteral.format` parses its template at compile time
    (`_FormatUtils.format_to_comptime[StaticString(Self())]`) and fails a
    `comptime assert` with the parse error.
  - Mojito's stand-in, `_FormatUtils.format_literal` in
    `stdlib/std/string.mojo`, parses at run time and aborts with the same
    message; a path the run never takes is accepted.
  - Ledger: `format-literal-template-runtime`
    (`conformance/fixtures/format_literal_template_runtime.mojo`).
  - The stand-in and the checker's `LiteralFormat` adjustment go away when
    `StringLiteral` declares upstream's `format`.
  - Found while landing R421 (2026-10-06).
  - Depends on R213.
  - Model: Fable, Not Planned.

- [ ] **R514 `rebind` between distinct structs of one lowered layout is
  rejected, where the pin accepts it**

  Problem: `rebind[S](x)` with `x: U`, where `U` and `S` each hold one
  `Int` field under any names, fails the instance in Mojito ("rebind input
  type 'U' does not match result type 'S'"), while the pin runs it by
  reference and by value.
  - The pin's `kgen.rebind` compares lowered KGEN types; Mojito's
    `discharge_rebinds` (`mono/rebind.rs`) compares checked types with
    `types_agree`, which is nominal.
  - The pin still rejects a different layout (`UInt` or `Float64` against
    `Int`, `String` against `S`), as Mojito does.
  - Following the pin needs a lowered-type identity in the elaborator,
    from `native-core`'s layouts, which judges the value and place forms
    alike.
  - Pinned by `conformance/probes/rebind_layout_identical_struct.mojo`.
  - Ledger name: `rebind-layout-identical-struct`.
  - Found while landing R314 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R515 Assigning through a whole `rebind` destroys the old value at
  the operand's type, where the pin destroys it at the target's**

  Problem: `rebind[S](x) = S(9)` with `x: T`, `T: Copyable`, is rejected
  in Mojito ("'x' abandoned without being explicitly destroyed"), while the
  pin prints `9`.
  - The pin assigns through the reference its `ref` overload returns,
    typed `S`, so the old value is destroyed as an `S`.
  - Mojito's `erase_rebinds` rewrites the statement into the plain
    assignment `x = S(9)`, whose destruction is judged at `T`, which is not
    `Deinitable`.
  - Following the pin means lowering the statement as a store through the
    rebound place (its `Proj::Rebind` step) rather than rewriting it.
  - Pinned by `conformance/probes/rebind_assignment_without_deinitable_bound.mojo`.
  - Ledger name: `rebind-assignment-without-deinitable-bound`.
  - Found while landing R314 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R511 A negative literal index into an `Array` traps at run time
  where the pin rejects it at compile time**

  Problem: `var a = [1, 2, 3]; print(a[-1])` fails to compile at the pin
  ("constraint failed: negative indexing is not supported"), while Mojito
  compiles it and traps with "Pointer access out of bounds".
  - Upstream dispatches a subscript whose index is a compile-time constant
    to `Array.__getitem_param__[idx]`, whose `comptime assert` rejects a
    negative or out-of-range index.
  - Mojito's bundled `Array` has no `__getitem_param__`, and the checker
    never routes a constant subscript to one.
  - Pinned by `conformance/probes/array_negative_literal_index.mojo`.
  - Ledger name: `array-negative-literal-index`.
  - Found while landing R274 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Mojito-Specific Shortcuts To Move Toward Mojo's Shape

Track: `mojo-shape`.

Parts of Mojito's stdlib lean on the Rust runtime where upstream is pure Mojo,
or take a Mojito-specific shortcut where upstream has its own shape. Each entry
below is a candidate port toward Mojo's shape, and a port is preferred over any
new bridge (2026-09-07 direction). The ports are independent of each other, and
an entry that waits on another names it. Four runtime services are
deliberately not on this list; they are in [`docs/non-goals.md`](non-goals.md).

- [ ] **R523 A local implicitly-copyable compile-time binding materializes at its declaration**

  A local `comptime s = label(3)` stores one runtime slot where Mojo materializes each read.
  - Requested collections already lower without a binding slot.
  - Move implicitly-copyable bindings to the same parameter-expression read path.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R524 Compile-time values still have two materialization mechanisms**

  Top-level strings and string-containing aggregates use constructors while frozen pointer memory uses backend constants.
  - Unify these at the typed constant boundary, matching Mojo's materialization operation.
  - Nominal VM-evaluated collections already use frozen storage without constructor replay.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R306 A scalar `range(...)` is typed by a checker rule where the
  pin declares infer-only overloads**

  Problem: `range(Int32(4))` is typed by `Checker::infer_scalar_range`
  once overload selection fails, which spells the call as the range
  struct's construction, where the pin's `std.builtin.range`
  declares `def range[dtype: DType, //](end: Scalar[dtype]) ->
  _ZeroStartingRange[dtype]` and its two- and three-argument siblings.
  - Mojito's float range is a separate `_FloatStridedRange`; the pin's
    `_StridedRange[dtype, forward]` takes the float path under `comptime if
    Self.dtype.is_floating_point()`.
  - The overloads reject a float or `Bool` lane with `comptime assert`,
    which Mojito does not parse (R74).
  - With the overloads in source, `infer_scalar_range`'s spelled
    construction and the checker's range shortcuts (`scalar_range_parts`
    in `iteration.rs`, `indexing.rs`, and `builtins.rs`) go.
  - Depends on R74.
  - Model: Opus, Planned.

- [ ] **R321 A `comptime for` iterates by a built-in rule, not by its
  iterable's iterator**

  Problem: Mojito unrolls a `comptime for` over a `range`, a compile-time
  list, set, or dictionary, or a pack by a rule of its own
  (`CtValue::comptime_iteration_elements`, `ComptimeSequence`), and rejects
  any other iterable with "'comptime for' iterates a range, a compile-time
  list, or a pack", where the pin runs a user `Iterator & Copyable`
  (`comptime for x in Count(3)` prints `2`, `1`, `0`).
  - Upstream's parser desugars every `comptime for` to `seq.__iter__()` in
    the parameter domain, and the elaborator steps it through the stdlib
    stubs `paramfor_has_next`, `paramfor_next_iter`, and
    `paramfor_next_value` (`std/builtin/_stubs.mojo`).
  - Following it moves the MIR header's binder from the element to the
    iterator, the element bound as `paramfor_next_value(it)`, with each
    step a compile-time application the elaborator runs.
  - Depends on R246.
  - Model: Fable, Not Planned.

- [ ] **R492 `ParameterList.get_span` addresses its elements through a
  compiler-private stand-in**

  Problem: the bundled `ParameterList` takes its elements' static address
  from `__param_list_address[*Self.values]()` (`MirInstr::ParamListAddress`),
  where upstream's body is `global_constant` over the list's
  `#pop.variadic_to_array` and an `unsafe_bitcast` of the array's address.
  - Porting upstream's body needs `std.builtin.globals` and an inline,
    trivially copyable `Array`; the bundled `Array` keeps its elements
    behind an `UnsafePointer`.
  - The stand-in is admitted only in `std/builtin/variadics.mojo`.
  - Found while landing R325 (2026-10-08).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R494 A compile-time read of a value pack is a checker rule, not a
  `ParameterList` member**

  Problem: `len(values)`, `values[i]` at a compile-time index, and `comptime
  for v in values` are recorded by the checker as parameter constants,
  where upstream answers them through `ParameterList.size` and
  `__getitem_param__` on the same struct a runtime read constructs.
  - Only a runtime read is the `ParameterList` construction today
    (`Checker::infer_value_pack_read`).
  - `ParameterList.size` waits on a readable struct `comptime` member over
    the struct's own pack.
  - Found while landing R325 (2026-10-08).
  - Depends on R493.
  - Model: Fable, Planned.

- [ ] **R378 A function lifted for a compile-time expression evaluates
  again each local `comptime` binding it reads**

  Problem: `comptime L = [n, n * 2]` read by `comptime if L[0] == 3:` and by
  `range(len(L))` in `def f[n: Int]()` is evaluated once for each of the
  two, where the pin evaluates a `comptime` alias once per instance and
  every read names that value.
  - A thunk begins with the statement of each local `comptime` binding it
    reads that denotes no parameter expression (`thunk_prologue`, `mir.rs`):
    a display, and any value that is not arithmetic over the binders
    (`comptime b = n > 2`, `comptime t = (L[0], L[1])`).
  - `materialize[L]()`, and a value read off a display that is no `Int` or
    `Bool`, build the display again where they cross
    (`Flatten::build_display`).
  - The bindings served are pure, so the values agree with the pin's and
    only the work is repeated.
  - Bind each such binding's one application in the instance, and read an
    element of it as a parameter expression over that value.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R389 One compile-time expression is lifted as more than one
  function**

  Problem: `comptime if h(n) > 4:` and `flag[h(n) > 4]()` in `def f[n:
  Int]()` evaluate two functions for the one expression, where the pin holds
  one parameter expression and every read names it.
  - The check names the function of an expression in a type or a parameter
    argument, or bound by a local `comptime` (`Checker::lifted_application`,
    `checker/comptime_validation.rs`).
  - MIR names its own for a condition, a `range` bound, and a runtime read
    of a display (`ComptimeThunks::request_value`, `mir.rs`).
  - An operand the check lifted before the whole expression failed to
    compile keeps its function too: `flag[h(vals[0]) > 2]()` lifts
    `h(vals[0])` and the comparison.
  - The values agree, and an unread function is never instantiated. Only
    the source MIR and the elaborator's work grow.
  - Let the check name every lifted expression, and drop a function nothing
    applies.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R159 `Float64` and `Float32` print different text from the pin
  because Mojito formats floats in Rust rather than in Mojo**

  Problem: upstream formats `Float64` in Mojo (Dragonbox in `format_float`),
  while Mojito formats it in Rust: the float arm of the VM's `Display for
  Value` and the native `mjrt_fmt_f64`.
  - Five corpus fixtures print different text on the two compilers because
    of it (`conformance/assets-mojo-output-diffs.tsv`, family
    `float-format`).
  - The pin writes an exponent sign: `1e+23`, where Mojito writes `1e23`.
  - The pin renders a `Float32` at its own precision: `0.1`, where Mojito
    writes `0.10000000149011612`.
  - The lever: transliterate a permissively licensed Rust Dragonbox (MIT or
    Apache-2.0 — third-party crates are allowed, see `AGENTS.md`) into Mojo
    rather than deriving the algorithm.
  - The plan picks the source and pins the shortest-round-trip cases.
  - Only a from-scratch derivation would want Fable.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R160 `DType` is a compiler builtin in Mojito where upstream's is a
  stdlib struct over a one-byte code**

  Problem: Mojito's `DType` is `Ty::Dtype`, with its `is_*` queries and
  display built into the VM and the native lowering. Upstream's `DType` is a
  stdlib struct over a one-byte code.
  - A bundled `struct DType` needs struct-valued associated `comptime`
    members, which `eval_associated_ct` rejects today.
  - It also needs runtime reads of `StructName.NAME`.
  - It also needs struct value parameters on defs.
  - It also needs a bridge from a frozen struct value to `Dtype` at every
    hard-wired `DType` site.
  - Several checker and elaborator capabilities land before the struct can
    replace the builtin.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R324 `reflect[T]` is a compiler builtin where upstream's is a
  stdlib struct over parameter attributes**

  Problem: Mojito answers `reflect[T]` queries by built-in rules in the
  checker (`checker/reflection.rs`), the crossing pass, `comptime/eval.rs`,
  and the elaborator (`mono/symbolic.rs`), where upstream's `Reflected[T]`
  (`std/reflection/reflect.mojo`) is a stdlib struct whose methods return
  parameter attributes its elaborator folds.
  - `field_count()` is `_field_types_of[T]().length` over
    `#kgen.struct_field_types`, `is_struct()` is `#kgen.is_struct_type`, and
    `field_index[name]()` is `#kgen.struct_field_index_by_name`.
  - The shared policy is one function today (`ReflectQuery::answer`,
    `param_expr.rs`), so the answers agree; the shape does not.
  - Closing it means a bundled `Reflected` over intrinsics that lower to
    `ParamKind::Reflect`, with the built-in rules deleted.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R276 `SIMD` writes its text in the host where upstream's writes it in
  Mojo**

  Problem: `print` and `repr` of a `SIMD` value are built into the VM
  (`format_value`, `simd_repr` in `backend/vm/dispatch.rs`) and the native
  lowering (`lower_repr_builtin`, `print_simd`). Upstream's `SIMD` is a
  stdlib struct whose `write_to` and `write_repr_to` are Mojo, choosing the
  scalar alias through `_scalar_repr_alias`.
  - The texts match the pin; the shape does not.
  - A bundled `struct SIMD` needs everything R160 needs for `DType`, plus
    `Ty::Simd`'s slots served by a nominal struct.
  - Depends on R160.
  - Model: Fable, Not Planned.

- [ ] **R359 `Tuple`'s private storage destroys its elements, where
  upstream's storage is trivially destructible**

  Problem: Mojito's `Tuple` holds a `__RuntimeTuple[*Self.Ts]` whose
  destruction destroys every element, where upstream's `!kgen.struct`
  storage has a trivial destructor and `Tuple.__deinit__` destroys each
  element through `Pointer(to=self[i]).unsafe_deinit_pointee()`.
  - The consuming members (`reverse`, `concat`, `consume_elements`) take
    each element with `Pointer(to=self[i]).unsafe_take_pointee()` and then
    must end the storage with `lit.ownership.mark_destroyed`, which upstream
    does not need.
  - Upstream writes the moves as `unsafe_write_move_from` over a rebound
    pointer, which Mojito does not have.
  - `unsafe_take_pointee()` through an origin-bearing pointer to an element
    with a destructor is admitted only in the bundled storage modules
    (`checker/method_calls/builtin_types.rs`), because user code cannot end
    the owner's storage.
  - Depends on R341.
  - Model: Fable, Planned.

- [ ] **R360 `TString` owns a `Tuple` of its parts, where the pin's borrows
  a pack beside its encoded literal text**

  Problem: Mojito's `TString[*Ts: Movable & Writable]` stores every literal
  segment and every interpolation in an owned `Tuple[*Self.Ts]`, where the
  pin's `TString[origins: ImmOrigin, //, *Ts: Writable]` borrows a
  `VariadicPack` of the interpolations and keeps the literal parts
  NUL-encoded beside it (`__make_tstring`).
  - An interpolated place whose type is not `ImplicitlyCopyable` is
    formatted to a `String` when the t-string is created, where the pin
    borrows it.
  - Mojito's `__make_tstring[*Ts](var *args: *Ts)` takes the literal
    segments as interleaved `String` arguments, where the pin's takes one
    `format_string` parameter beside the interpolations.
  - The pin's shape needs an origin-carrying struct over a borrowed pack.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R341 `Tuple`'s pack, copy, and move initializers store the private
  storage whole where upstream marks `self` initialized and writes each
  element through a pointer**

  Problem: Mojito's `Tuple.__init__(out self, var *args)` builds
  `self.storage = __RuntimeTuple(*args^)`, and its copy and move
  initializers are synthesized over the whole storage. Upstream marks
  `self._mlir_value` initialized and writes per element:
  `args^.consume_elements[init_elt]()` with
  `Pointer(to=self[idx]).unsafe_write(elt^)`, `unsafe_write(copy=copy[i])`,
  and `unsafe_write_move_from(Pointer(to=move[i]))`.
  - The default initializer took upstream's shape on 2026-10-05: the
    `lit.ownership.mark_initialized` statement and
    `Pointer(to=self[i]).unsafe_write({})`.
  - Mojito has no `unsafe_write_move_from`.
  - A `@__parameter` closure over the receiver (`init_elt`) is not spelled
    in Mojito.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R36 `Layout` carries its alignment as a runtime field**

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

- [ ] **R86 `String` always owns a heap buffer, where upstream's has three
  representations**

  Problem: `String(literal)` allocates and copies the literal's bytes, while
  upstream points at the static bytes and copies only on the first mutation.
  - Upstream packs a static-constant, an inline (up to 23 bytes), and a
    reference-counted heap form into the same 24 bytes, flagged in
    `_capacity_or_data`; Mojito's `{data, size, cap}` has only the heap form.
  - No program output differs; allocation counts and `capacity()` do.
  - Port the static form first: every mutator, `__del__`, copy, and move
    must respect a non-owning flag, on both backends.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R330 SIMD's lane gathers are checker intrinsics where the pin's
  are `SIMD` methods over `pop.simd.shuffle`**

  Problem: `shuffle`, `slice`, and `join` are typed by rule in
  `simd_receivers.rs`, and their constraints are checked by
  `LaneMask::resolve`, where the pin declares them in `std/simd.mojo` and
  checks them with `comptime assert`.
  - Join's template mask is the structured `LaneMask::Join`, where the
    pin's is the compile-time result of `indices()`.
  - The pin's `slice` lowers to `llvm.vector.extract` or an element loop,
    not a shuffle.
  - A self-hosted `SIMD` would move the typing, the constraints, and the
    lowering choice into source, and `LaneMask` would shrink to the known
    mask.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R336 `SIMDLength` is `Int` in Mojito, told apart only where a width
  binder is inferred**

  Problem: upstream declares `SIMD[dtype: DType, length: SIMDLength]`, so a
  vector argument solves a `w: SIMDLength` binder and leaves an `n: Int` one
  unresolved; Mojito types both as `Int` and records the `SIMDLength`
  binders in a checker side table (`Checker::simd_length_binders`) that
  only the solver reads.
  - Behavior matches the pin today: `SIMD[_, _]`'s width binder, a declared
    `w: SIMDLength`, and the `H: Hasher` requirement's width solve, and an
    `Int` binder in the slot is rejected
    (`assets/type_error/simd_int_width_binder_not_inferred.mojo`).
  - Upstream's shape is a distinct `SIMDLength` type with an implicit
    conversion from `Int`, so the declaration, not a side table, carries it.
  - Found while landing R259 (2026-10-05).
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R346 The discovery check types `Tuple`'s dunders and teardowns in
  Rust though `tuple.mojo` declares them**

  Problem: `infer_tuple_method`
  (`checker/method_calls/builtin_types.rs`) still types `__len__`,
  `__contains__`, the six comparisons, `consume_elements`, and `deinit_with`
  from the element list for a `Tuple` no specialization serves yet.
  - `reverse` and `concat` are typed against the declaration's shell
    (`infer_tuple_member`), and the same path can answer the rest.
  - The Rust arms answer first, so removing one moves its member to the
    declaration.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R422 `print` and `String(x)` write a struct through a host string
  accumulator where upstream writes through a `Writer` struct**

  Problem: a struct's `write_to` reached from `print` or `String(x)` gets a
  compiler-private string accumulator as its writer, where upstream passes a
  real `Writer` (the stdout buffer, or the `String` itself).
  - A `writer.write(...)` call inside such a body dispatches through the
    bound to `__trait_dispatch.write`, which the specializer clears on the
    accumulator's non-struct receiver (`native/mono/specializer.rs`).
  - The VM answers it in the `Value::Str` arm of `method_call`
    (`backend/vm/invoke.rs`) and Pliron in `lower_str_writer_write`
    (`lower/methods.rs`).
  - A writer that overrides `write` is never the accumulator, so the texts
    match the pin; the shape does not.
  - Found while landing R414 (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R424 `Writable` is a compiler builtin where upstream declares it in
  `std/format`**

  Problem: `Writable` and its `write_to`/`write_repr_to` requirements are
  synthesized in the checker (`builtin_requirements`, `checker/traits.rs`),
  where upstream's `std/format/__init__.mojo` declares the trait beside
  `Writer`, with Mojo default bodies.
  - The reflective `Name(field=value, ...)` default the checker synthesizes
    for a conformer without a `write_to` is upstream's default body.
  - `Writer` moved to the bundled `std/format/__init__.mojo` with R414, so
    this is the remaining half of that module.
  - Found while landing R414 (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R426 The bundled `_FormatUtils` respells constructs upstream writes
  differently**

  Problem: the port of upstream's `collections/string/format.mojo` in
  `stdlib/std/string.mojo` matches the pin's behaviour but not its text,
  because Mojito rejects several of upstream's spellings.
  - `_FieldVariantType = Variant[StringSlice, Int, NoneType, Bool]` is an
    `Int` kind tag plus an `Int` value, the kinds module-level `comptime`
    constants (R428, R436).
  - `ord("{")` and friends are `UInt8(123)` runtime locals (R427), and the
    `supported_conversion_flags` SIMD membership test is two comparisons
    (R135).
  - `Error("Index ", val, " not in *args")` concatenates (R429), and the
    `comptime l_err`/`r_err` texts are inlined (R435).
  - The nested `_format` closure is a method of `_FormatCurlyEntry` (R288),
    and its `ref arg = args[i]` reads `args[i]` at each use (R438).
  - `Int(field)` is `atol(String(field))` (R430), `fmt_bytes.unsafe_get(i)`
    is `fmt_bytes[i]` (R431), and `_build_slice` is a `[byte = a : b]`
    slice.
  - `x and bytes[i] == …` and `manual and automatic` (Int truthiness) are
    nested `if`s and `!= 0` tests (R97); `for i in range(n)` is a `while`
    loop (R243); a `raised_kwarg_field = String(f)` store spells
    `Optional[String](…)` (R437).
  - `_PrecompiledEntriesRuntime` holds no `format` view and no origin
    parameter: a struct field over a `def`'s auto-parameterized
    `StringSpan` origin does not bind, so `format_precompiled` takes the
    template beside the entries. The span-backed `_PrecompiledEntries`,
    `format_to_comptime`, and `compile_entries_runtime_no_raises` are
    omitted until R425.
  - Found while landing R421 (2026-10-06).
  - Depends on R427, R428, R429, R430, R431, R435, R436, R437, R438, R288,
    R135, R97, R243.
  - Model: Opus, Not Planned.

- [ ] **R445 `ConstructTypeParam` is a special case of `ConstructType`**

  Problem: `T()` and `Ts[i]()` are carried by `MirInstr::ConstructTypeParam`
  (a binder, plus a pack index), while `types[i]()` is carried by
  `MirInstr::ConstructType` over the type expression, where the pin has one
  operation: the `Defaultable` initializer of a type-valued parameter
  expression.
  - Folding the nullary forms gives `T()` the operand `Ty::Param` and
    `Ts[i]()` the dependent selection, closed by the elaborator's one
    `ConstructType` arm (`mono::substitute::default_construct_parameters`).
  - The erased VM constructs those two forms from type reification by slot
    name, so the fold waits for the erased oracle's removal.
  - `T(copy=x)` keeps its own form, or becomes a selected `Copyable`
    initializer call.
  - Found while landing R364 (2026-10-07).
  - Depends on R10, which deletes the erased oracle's reification.
  - Model: Opus, Not Planned.

### Grow The CPU Standard Library *(demand-first)*

Track: `stdlib`.

The standard-library surface grows on demand toward the audited head: the
tuple, slice, optional/variant, and String surfaces, and the filesystem and
I/O residues left behind the landed files, streams, paths, and tempfile stage.
`docs/features.md` records what lands. Each entry below is a conscious,
recorded limit, listed in impact order: soundness of the executable oracle
first, then everyday spellings that reject today, then parity details. A
residue found inside a task moves to the task that owns its fix.

- [ ] **R197 Compile-time Dict/Set key identity ignores a user struct's
  non-fieldwise `__eq__`**

  Problem: compile-time Dict/Set key identity is structural `CtValue`
  equality, which diverges from upstream for a user-struct key with a
  non-fieldwise `__eq__`.
  - Structural equality is exact for every prelude key type.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R198 The typing probe re-checks the CTFE subprogram for every
  VM-bound expression**

  Problem: the typing probe checks the CTFE subprogram once more per
  VM-bound expression, a compile-time cost upstream does not pay.
  - No lever is named.
  - Depends on nothing.
  - Model: Fable, Not Planned.

- [ ] **R199 A string element of a compile-time tuple indexed under
  `comptime for` breaks inside a list display**

  Problem: with `comptime t = (1, "s")` and `comptime for i in range(2)`,
  `first([t[i], t[i]], t[i])` fails MIR verification with `register r19 has
  no checked type`.
  - `var v = t[i]` followed by `first([v, v], v)` fails with `binding of
    StringLiteral to a slot of type String`.
  - The `Int`/`Bool` twin runs.
  - Cause: the materialized element keeps its literal type where the
    display's element type has already materialized `String`.
  - Two `var` declarations across unrolled iterations also collide (`'v' is
    already declared in this scope`) unless each body opens a block. That
    collision is R40's to fix.
  - Depends on R40, whose `comptime for` scoping fix lets the `var v` repro
    run without a block per body.
  - Model: Opus, Not Planned.

- [ ] **R493 The bundled `ParameterList` lacks most of upstream's members**

  Problem: `ParameterList.of[4, 5, 6]`, `ParameterList[values.values]`,
  `args.size`, `print(args)`, and the list algebra are rejected, where the
  pin's `std/builtin/variadics.mojo` declares them.
  - The port's list is the variadic `*values: type` upstream's
    `_ParameterListIter` spells, not upstream's single
    `KGENParamListType[type]` parameter, so `values.values` and the `of`
    and `empty_of` aliases have no spelling.
  - Missing members: `size`, `__getitem_param__`, `of`, `empty_of`,
    `tabulate`, `splat`, `_concat`, `reduce`, `any`, `all`, `contains`,
    `map_to_type`, and the `Writable` conformance (`write_to`,
    `write_repr_to`).
  - The port reads `len(Self.values)` where upstream reads `Self.size`.
  - Found while landing R325 (2026-10-08).
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R204 An abstract dispatch target that survives elaboration aborts at
  run time**

  Problem: a call the checker emits as an abstract `__trait_dispatch.*` or
  `__iterator_dispatch.*` target that `native::mono` leaves unresolved
  reaches the VM, whose by-name retargeting then aborts, where the
  elaborator should reject the program.
  - The VM keeps the retargeting (`TRAIT_DISPATCH_PREFIX`,
    `symbol::borrowed_iterator_dispatch_alternate`; `backend/vm/exec.rs`,
    `invoke.rs`) only for such a survivor; a target the elaborator resolves
    (`mono/infer.rs`) never reaches it.
  - Sites: an operator or protocol dunder reached from a value whose static
    type is a bare `Ty::Param` the instance did not close; an instance
    reached only from bundled code.
  - Each survivor should be a `MonoError` at the call that reaches it, as the
    pin rejects an unsatisfied requirement; the retargeting goes with the
    last one.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R206 Type-pack calls inside a nested `def` and whole-pack-forwarded
  calls keep the syntactic element-typing path**

  Problem: `take(b, 1)` over a local `b` rejects with `a heterogeneous pack
  specialization needs an expression whose type is statically evident before
  checking`.
  - Top-level calls consult the checker's instantiation.
  - Three of the four pieces exist for ordinary nested generics: an
    unresolved template survives `replace_templates` for the discovery
    check.
  - `def_specialization_requests` harvests a nested callee.
  - `NestedMono::scan_expression` consults that request.
  - Remaining: the checker accepting a nested variadic shell abstractly.
  - Remaining: softening the pack diagnostics. Only an arity failure defers
    today, and a pack failure is `PackBound` or `NotComptime`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R207 A user variadic struct application as a pack element keeps the
  fixed-arity diagnostic**

  Problem: `Tuple[TypeNames[Int]]` rejects with the fixed-arity diagnostic.
  - Its erased shell has no sound nominal form.
  - The public `Tuple` is the one compiler-known template whose `*Ts`
    absorbs every argument.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R208 A public Tuple as an explicit type argument does not conform
  to `Deinitable`**

  Problem: `make[T: Defaultable & Deinitable]()` over `Tuple[Int, Bool]`
  reports the bound failure.
  - The inferred shape runs.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R343 `Tuple.reverse` and `Tuple.concat` move each element by a
  transfer out of the private storage where upstream moves it through a
  pointer**

  Problem: both are declared in `std/builtin/tuple.mojo` with upstream's
  signatures, but their bodies write
  `Pointer(to=result[i]).unsafe_write(self.storage[k]^)`, where upstream
  writes `Pointer(to=result[i]).unsafe_write_move_from(rebind[Pointer[
  type_of(result[i]), origin_of(self)]](Pointer(to=self[k])))`.
  - A move out through a pointer is untracked, and the compiler destroys the
    `__RuntimeTuple` storage, so the element would be destroyed twice.
  - `concat` retypes the destination place instead of the source pointer:
    `rebind[Self.Ts[i]](result[i])`.
  - `concat` reads its length as `Self.Ts.length` where upstream calls
    `Self.__len__()`.
  - The loop headers read `Self.Ts.length` and `OtherTs.length` where
    upstream reads `type_of(result).__len__()` and
    `type_of(other).__len__()`.
  - Pinned by `assets/ok/tuple_reverse_concat.mojo`.
  - Depends on R341 (the storage model and `unsafe_write_move_from`), R134
    (`type_of`), R340 (`Self.__len__()` in a header), and R344.
  - Model: Fable, Planned.

- [ ] **R210 A lambda parameter annotated `String` reports an unknown
  type**

  Problem: `lambda (s: String): print(s)` reports `unknown type 'String'`,
  while a nested `def` with the same parameter runs.
  - Likely cause: the lambda's hidden `def` misses the prelude qualification
    the other annotations get.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R211 A list display passed to an explicitly applied constructor
  takes no context from the parameter type**

  Problem: at runtime `Dict[String, Int](["a"], [1], None)` takes no context
  from the parameterized `List[Self.K]` parameter, materializes the displays
  as `Array`, and matches no overload.
  - Workaround: spell the lists (`List[String]("a",
    __list_literal__=None)`), as a compile-time value's materialization
    does.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R212 A struct's own value parameter as a SIMD width in a signature
  is rejected as a type**

  Problem: `def zeros(self) -> SIMD[DType.int64, Self.length]` reports `SIMD
  width must be a positive power of two, got a type`, where upstream binds
  the parameter.
  - Cause: the width slot needs a concrete comptime `Int`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R213 String literals have no methods**

  Problem: `"abc".byte_length()` rejects.
  - Workaround: convert a literal-typed value through `String(...)` first.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R214 An annotated `Optional[T]` local initialized from a bare
  struct construction fails MIR verification**

  Problem: `var o: Optional[P] = P(4)` fails MIR verification with an
  untyped register for the constructor call.
  - Workaround: spell the wrapper (`Optional[P](P(4))`, `Optional(P(4))`).
  - A literal payload (`var n: Optional[Int] = 6`) converts.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R216 A string literal does not bind a trait-bounded type parameter
  that `String` satisfies**

  Problem: `isdir("/tmp")` reports `'StringLiteral' ... does not conform to
  trait 'PathLike'`.
  - Workaround: spell `String("/tmp")`.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R217 Struct-level `comptime NAME = Self(n)` constants do not
  fold**

  Problem: upstream's `ErrNo.ENOENT` shape does not fold.
  - Cause: the associated-constant evaluator handles prefix, infix, tuple,
    and list expressions only.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R218 `@fieldwise_init` beside a hand-written `__init__` is
  rejected**

  Problem: a struct carrying both `@fieldwise_init` and a hand-written
  `__init__` rejects.
  - Workaround: spell every constructor by hand.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R219 A bound-generic def whose template body calls itself with a
  concrete argument reports its own specialization undefined**

  Problem: `makedirs(head, exist_ok=...)` inside `makedirs[PathLike]` mints
  its own specialization while checking and reports it undefined.
  - Workaround: recurse through a non-generic helper.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R220 Nullary construction of a sized-scalar array is reported
  unsupported**

  Problem: `Array[Int8, 1024]()` reports the scalar's `Defaultable`
  construction unsupported.
  - Workaround: spell `Array[Int8, 1024](fill=0)`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R221 `Pointer.unsafe_bitcast[U]()` leaves its result at the old
  element type**

  Problem: `Pointer.unsafe_bitcast[U]()` is not typed: an origin-cast-style
  forwarding leaves the MIR register at the old element type.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R222 `CStringSlice` views `Byte` elements instead of `c_char`**

  Problem: upstream's `CStringSlice` views `c_char` elements, Mojito's views
  `Byte`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R223 `std.builtin.rebind.downcast` is missing, so `Dict`/`Set`
  views are guarded by `Copyable` instead**

  Problem: with no `downcast`, `Dict`/`Set` guard `keys`, `values`, `items`,
  and `__iter__` with `where conforms_to(K, Copyable)` instead of laundering
  the parameter.
  - `Dict.keys` needs copyable values as well as keys, since the key view
    wraps the entry view.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R224 Nested struct names print unqualified where upstream keeps the
  module path**

  Problem: `_unqualified_type_name` spells nested structs unqualified where
  upstream keeps a non-prelude struct's module path
  (`Optional[std.collections.dict.Dict[...]]`, `List[up.Flag[True]]`).
  - `List`, `Optional`, `String`, and `SIMD` stay bare.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R225 Grapheme segmentation covers only a UAX #29 essentials
  subset**

  Problem: segmentation is the documented UAX #29 essentials subset, with
  hand-maintained Control/Extend/SpacingMark ranges and no
  Extended_Pictographic or Prepend data.
  - Reverse iteration re-scans forward from the nearest CR/LF/Control
    boundary.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R226 Public `stat` / `lstat` / `stat_result` are not ported**

  Problem: the filesystem stage left public `stat`, `lstat`, and
  `stat_result` unported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R227 `realpath` is not ported**

  Problem: the filesystem stage left `realpath` unported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R228 `symlink` / `link` / `chdir` are not ported**

  Problem: the filesystem stage left `symlink`, `link`, and `chdir`
  unported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R229 `isatty` is not ported**

  Problem: the I/O stage left `isatty` (`FileDescriptor.isatty` / `fchdir`)
  unported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R230 `OptionalPointer` is missing**

  Problem: upstream's `OptionalPointer` is not ported, so null tests spell
  `Int(ptr) == 0`.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R231 `ErrNo`'s named constants are missing**

  Problem: `ErrNo` has none of upstream's named constants (`ErrNo.ENOENT`).
  - A small port.
  - Depends on R217, which folds the struct-level `comptime NAME = Self(n)`
    shape the constants use.
  - Model: Opus, Not Planned.

- [ ] **R232 `~user` path expansion is not supported**

  Problem: `~user` expansion (`getpwnam`) is not ported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R233 The VM has no per-process environment overlay**

  Problem: the VM lacks a per-process environment overlay, while native
  `setenv` writes the real environment.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R234 `NamedTemporaryFile` is not ported**

  Problem: the tempfile stage left `NamedTemporaryFile` unported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R235 `FileHandle.read` cannot read into a typed span**

  Problem: `FileHandle.read` into a typed `Span[Scalar[dtype], origin]` is
  not ported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R236 `Path.stat` / `lstat` / `_dir_of_current_file` are not
  ported**

  Problem: the paths stage left `Path.stat`, `Path.lstat`, and
  `_dir_of_current_file` unported.
  - A small port.
  - Depends on R226, which ports the `stat` / `lstat` / `stat_result` they
    wrap.
  - Model: Opus, Not Planned.

- [ ] **R237 `KeyElement` is missing**

  Problem: upstream's `KeyElement` is not ported.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R238 `_get_random_name` reads `/dev/urandom` instead of using
  `std.random`**

  Problem: `_get_random_name` reads `/dev/urandom` today, where upstream
  builds it over `std.random`.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R239 There is no generic `__exit__[E]`**

  Problem: the context-manager surface lacks upstream's generic
  `__exit__[E]`.
  - A small port.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R240 An overloaded constructor spelled with the `StringSlice` or
  `Byte` alias mangles a different key than the call selects**

  Problem: an overloaded constructor spelled with the `StringSlice` or
  `Byte` alias mangles a different key than the `StringSpan` / `UInt8` the
  call selects.
  - Workaround: the ports spell the canonical names.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R241 A temporary view used as a method argument or
  augmented-assignment operand fails in the VM**

  Problem: `s.take(String("x").as_bytes())` is a VM "reference receiver must
  be a place" rejection.
  - As an augmented-assignment operand (`p /= StringSpan(s)`) it is a VM
    use-after-free.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R242 `Span[mut=True, T, _]` fails origin inference**

  Problem: a parameter typed `Span[mut=True, T, _]` fails origin inference.
  - Workaround: spell an `[origin: Origin[mut=True]]` binder.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R243 `range` is invisible in `std.string`**

  Problem: code in `std.string` cannot see `range`.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R244 A module loaded while the prelude bootstraps must import
  `String` explicitly, and the graph slows Hello World**

  Problem: a module loaded while the prelude bootstraps (`std.io` and the
  whole `std.os` graph now) must import `String` explicitly.
  - That graph costs Hello World about a second of debug compile time
    (`docs/performance.md`).
  - No lever is named.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R143 Scalars have no comparison methods**

  Problem: `x.ne(y)` on a `Float64` (or any width-1 scalar) is rejected with
  `type 'Float64' has no method 'ne'`, though the pin accepts it.
  - Upstream's scalars are width-1 `SIMD`, so `lt`/`le`/`gt`/`ge`/`eq`/`ne`
    exist on them too. Mojito resolves those methods only on a multi-lane
    `Ty::Simd` receiver (`crates/mojito-checker/src/checker/method_calls/simd_receivers.rs`).
  - The methods must keep the multi-lane semantics: the pin's scalar
    `s.ne(s)` is `False` for a NaN (ordered), while infix `s != s` is `True`.
  - Not a wrong answer, only a missing spelling. Until then, `x < y or x > y`
    is the ordered `ne`; infix `!=` answers `True` for a NaN.
  - The `simd-infix-comparison` divergence waits on it: withdrawing the
    infix spelling leaves `x.ne(y)` as the only ordered comparison on a
    scalar.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R339 A scalar has no `__int__` method**

  Problem: `v.reduce_add().__int__()` on a `SIMD[DType.int32, n]` prints
  at the pin, while Mojito reports "type 'Int32' has no method '__int__'".
  - `Int(x)` converts the same scalar, through the conversion built-in,
    not a method (`checker/builtins.rs`).
  - Upstream's scalar is a width-1 `SIMD`, whose `Intable` conformance is
    an ordinary method.
  - Found while landing R4's struct-valued half (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R335 The native `Int` and `Float64` have no lane reductions, even as a
  `Scalar[dt]` instance**

  Problem: `v.reduce_add()` in `def total[dt: DType](v: Scalar[dt])` fails at
  run time with "vm backend does not support methods on Float64 yet" for
  `total(Float64(2.5))` (and on `Int` for `total(Int(3))`), and
  `Float64(2.5).reduce_add()` is rejected outright; the pin runs both.
  - Upstream's `Float64` is `Scalar[DType.float64]`, a width-one `SIMD`
    with every lane method, and its `Int` converts to `Scalar[DType.int]`.
    Mojito canonicalizes both width-one vectors to the native scalars, which
    `runtime::simd_method` and the checker's SIMD receiver arms skip.
  - A sized scalar (`Float32(1.5)`) at the same binder runs.
  - Found while landing R259 (2026-10-05).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R144 Time, random, and testing slices**

  Goal: deterministic testable cores, with host-dependent behavior behind
  runtime services.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R331 A one-operand `shuffle` rejects a lane index at or above the
  receiver's width, which the pin accepts**

  Problem: `SIMD[DType.int32, 2](5, 6).shuffle[3, 0]()` fails to check
  with "expected a lane index below 2", where the pin prints `[6, 5]`.
  - The pin's one-operand `shuffle` gathers from the receiver joined with
    itself and asserts only `0 <= mask[i] < 2 * Self.length`.
  - The checker's rule is in `infer_simd_shuffle`, and an instance's in
    `LaneMask::resolve`; both should take the pin's bound and fold an
    index `i >= w` to `i - w`.
  - `tests/checker_test.rs`'s `rejects_bad_simd_shuffle_masks` pins the
    current rejection.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R411 `Variant` stands a `comptime if` line where upstream calls
  `Self._check[T]()`**

  Problem: upstream's `Variant` methods open with `Self._check[T]()`, whose
  `comptime assert` rejects a `T` that is no alternative; Mojito's
  `stdlib/std/utils/variant.mojo` has a no-op
  `comptime if Self.Ts.contains[T](): pass` there instead.
  - The check itself holds: a storage operation's
    `_get_type_index[T, *Ts]()` fails the instantiation at such a `T` with
    upstream's message (`close_variant_index`, `native::mono`).
  - Port `_InvalidTypeIndex`, `_get_type_index`, and `_check` verbatim once
    `comptime assert` parses.
  - Depends on R74.
  - Model: Opus, Not Planned.

- [ ] **R427 `ord` is undefined**

  Problem: `ord("{")` returns `123` at the pin, while Mojito reports
  "Undefined variable 'ord'".
  - Upstream's `ord` lives in `builtin/string_literal.mojo` beside `chr`.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R428 `NoneType` is not a type name**

  Problem: `Variant[StringSlice, Int, NoneType, Bool]` builds at the pin,
  while Mojito reports "Undefined variable 'NoneType'".
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R429 `Error` has no variadic `Writable` constructor**

  Problem: `Error("Index ", 3, " not in *args")` builds the joined message
  at the pin, while Mojito reports "'Error' expects 1 argument(s), got 3".
  - Upstream declares `Error.__init__[*Ts: Writable](out self, *args: *Ts)`.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R430 `Int` does not parse a `StringSpan`**

  Problem: `Int(StringSpan("12"))` is `12` at the pin and raises on a
  non-number, while Mojito reports "expected a numeric or Bool value, found
  StringSpan".
  - Workaround: `atol(String(view))`.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R431 `Span` has no `unsafe_get`**

  Problem: `bytes.unsafe_get(i)` over a `Span[UInt8]` reads the element at
  the pin, while Mojito reports "type 'Span[UInt8]' has no method
  'unsafe_get'".
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R432 `StaticString` is an unknown type**

  Problem: a parameter typed `StaticString` compiles at the pin, while
  Mojito reports "unknown type 'StaticString'".
  - Upstream's `StaticString` is `StringSlice[StaticConstantOrigin]`.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Opus, Not Planned.

### Packaging, Artifacts, And Developer Tooling

Track: `tooling`.

- [ ] **R530 `benchmarks/compile/tuple.mojo` is not valid Mojo**

  Problem: the pin rejects the benchmark (`p[0]` against `__getitem__[i: Int](self)`, and a non-`Deinitable` field), and Mojito now rejects it too.
  - `compiler_test::discovery_scan_matches_the_checked_arena` dropped it from its list.
  - Rewrite it in the pin's spelling (`__getitem_param__`, a `Deinitable` bound) and restore it.
  - Depends on nothing.
  - Model: Opus, Planned.

- [ ] **R145 The corpus sweeps no longer run in the overnight gate**

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
  - Model: Opus, Planned.

- [ ] **R146 Naming the bundled stdlib with `-I` breaks every program**

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
  - Model: Opus, Not Planned.

- [ ] **R147 Compile-time performance**

  Problem: Hello World is 1.5 s release / 7.9 s debug on the reference
  machine (`docs/performance.md`), and the first checker pass over the
  prelude is now most of it.
  - A body whose inputs are unchanged since the previous checker pass takes
    that pass's facts (`checker/body_carry.rs`), so the later passes of a
    compilation are cheap. The first pass still infers every prelude body.
  - `checked_var_types` scans the whole expression table per variable, and
    `explicit_destroy` re-derives deinitability per struct per pass.
  - Only then cache the elaborated/checked stdlib across processes.
  - R148, R149, and R150 are named costs under the same goal and land in
    any order.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R148 A carried checker pass still copies every fact it keeps**

  Problem: a pass that carries nearly every body still costs about a third
  of an inferring pass (`check_program.bodies` under `--timings`).
  - The carry clones each logged entry into the fresh checker's stores and
    hashes every site's syntax, once per pass (`body_carry.rs:carry_body`,
    `def_syntax_hash`).
  - Moving the previous pass's stores into the fresh checker and removing
    the entries of the bodies it infers would replace the copies.
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R150 Request identity names binding identities through origins**

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

- [ ] **R151 Feature and target options**

  Goal: checked CLI/build configuration recorded in artifacts and
  diagnostics.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R152 Compiled package artifacts**

  Goal: a versioned `.mojoc` representation (modules stay non-first-class).
  - Per-directory resolution order: source package, `.mojoc`, source module,
    legacy `.mojopkg`.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R153 Debugging metadata and inspection**

  Goal: stack/source diagnostics, MIR inspection, and debugger-oriented
  value rendering.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R154 Testing tools**

  Goal: Mojito-native assertions, expected-error tests, and
  differential-harness integration.
  - Depends on nothing.
  - Model: Fable, Planned.

- [ ] **R156 Three `comptime_test` callable-argument tests stop in
  elaboration**

  Problem: `specialization_retains_defaulted_thin_callable_arguments`,
  `nested_specialization_retains_capturing_callable_arguments`, and
  `callable_contract_binder_shadows_the_enclosing_binder` fail with an
  elaboration error since the VM runs concrete MIR.
  - The errors: "monomorphization cannot resolve parameter `callback`",
    "binding call to `main$add` during monomorphization: Missing(\"value\")",
    and a parameter type that "keeps symbolic type `def[T: Copyable &
    Deinitable](T) thin -> T`".
  - Each program ran on the erased VM. Whether the elaborator or the test's
    expectation is wrong is not yet known.
  - Found on `da941aee` while `comptime_test` was run for the struct-method
    templates (2026-10-01).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R157 Distribution reproducibility gate** *(last)*

  Goal: the release check rebuilds, tests, documents, and reproduces
  conformance from the crates.io archive alone.
  - It stays last whatever else moves.
  - Depends on every other entry in this track.
  - Model: Fable, Planned.

- [ ] **R439 The VM runs `format` about fifteen times slower than the Rust
  formatter it replaced**

  Problem: 10,000 `"{} and {}".format(i, "x")` calls take 35.6 s in a
  release build, against 2.3 s before `format` became the bundled Mojo
  port.
  - Each call parses the template in interpreted Mojo; an
    `Optional[Int]` round trip alone costs about 0.13 ms in the VM.
  - Found while landing R421 (2026-10-06).
  - Depends on nothing.
  - Model: Fable, Not Planned.

### Code Organization Follow-Ups *(behavior-preserving)*

Track: `code-org`.

The module splits (`docs/symbol-map.md`) removed every file over 3,000
lines. The rest needs semantic extraction, not line moves.

- [ ] **R158 Shrink the 2 kloc band** *(standing)*

  Split these further only along a cohesive seam, while touching them:
  - `checker/traits.rs` (2,629), `mir/lower_stmt.rs` (2,595),
    `checker/inference.rs` (2,520), `checker/statements.rs` (2,471),
    `ast.rs` (2,464), `mir.rs` (2,426), `checker/type_resolution.rs`
    (2,395), `runtime.rs` (2,235), `checker.rs` (2,232),
    `comptime/rewrite.rs` (2,179), `checker/declarations.rs` (2,118),
    `mir/text/write.rs` (2,116), `backend/vm/exec.rs` (2,085).
  - Depends on nothing. Split one of these only while touching it for
    another task.

- [ ] **R457 HIR types a slot from the first identifier of its name in the
  whole program**

  Problem: `checked_var_types` (`mojito-hir/src/hir.rs`) types each slot of
  a function's CFG from the first checked node anywhere in the program's
  expression arena whose syntax is an identifier of the slot's name.
  - A `comptime for` binder `p` over tuples was typed `Int` from an unrelated
    stdlib `p`; MIR's loop header now types its slot from the binder, but any
    other slot MIR does not retype keeps the stray type.
  - Restricting the scan to the function body's own nodes should change no
    current output, which is why the entry is here.
  - Found while landing R405 (2026-10-07).
  - Depends on nothing.
  - Model: Opus, Not Planned.

- [ ] **R521 `check_program_for_discovery` and `DiscoveryResult` carry the
  names of the deleted discovery rounds**

  Problem: the checker entry that stops before the checked arena is built,
  and its result type, are named for the discovery rounds R9 deleted.
  - Rename to `check_program_facts` / `CheckedFacts`; `tests/compiler_test.rs`
    and `docs/architecture.md` name both.
  - Depends on nothing.
  - Model: Opus, Planned.

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
  delete it here (no other entry moves or changes) and record the outcome in `docs/features.md`,
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
- Every entry is `- [ ] **R<n> Title**`, with `R<n>` reserved by
  `scripts/roadmap.py new-id`. Never renumber an entry, never reuse an ID,
  and never cite an entry by its position.
- Put an entry in the track that owns its fix, at the position its impact
  earns. Dependencies do not constrain position: the work order puts a
  prerequisite first wherever it sits.
- Every checkbox carries exactly one **Depends on** bullet, immediately
  before its **Model:** bullet: the IDs it needs, each with a short reason
  if one helps, or `Depends on nothing.` Name an ID in that bullet only as a
  dependency. A track's last entry may say `Depends on every other entry in
  this track.`
- Landing an entry deletes it and edits nothing else in this file: a
  Depends bullet that names a landed ID stays as it is.
- Run `scripts/roadmap.py lint` after editing this file. It checks IDs,
  tracks, the Depends and Model bullets, and dependency cycles.
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
