# Differential Probes

Minimal programs pinning an **open question, ambiguity, or known mismatch**
with current Mojo. Each file's header documents the question, both compilers'
expected behavior, and exactly what to update once the answer is known. Unlike
`../fixtures/`, these are **not** claims and are not listed in `cases.tsv` —
they are experiments to run by hand when a pinned Mojo build is available:

```sh
mojo run <probe>.mojo                # in the audited Pixi environment
cargo run -- run conformance/probes/<probe>.mojo
```

Record the answer by editing the parity/fixture files named in the probe's
header, then delete or repurpose the probe (promote it to a `cases.tsv`
fixture when it becomes a claim). Probes marked *re-probe* below duplicate an
already-enforced claim whose evidence predates the current audit head — run
them after every re-pin.

The `ae386d1b204` open-question pass (2026-08-15) resolved and deleted
seventeen probes; their answers live in `cases.tsv` claims
(`string-keyword-slice`, `optional-owning-surface`, `set-owned-iteration`,
`span-iteration-write-through`, `variant-owning-ops`, `insert-displacement`,
`string-positional-slice`, `string-literal-slice`, `owned-pointer-deref`,
`span-parameter-bare-element`, `len-string-units`, `subtree-origin-cast`,
`raise-caught-error`) and the parity rows they reference.

## Deprecation / vocabulary tracking (re-run at every re-pin)

Re-run 2026-08-26 against the `a79fbdf59f2` build (`Mojo 1.1.0.dev2026082605
(dd957314)`): `SIMDSize` and `TypeList.size` now REJECT upstream (`use of
unknown declaration 'SIMDSize'`; `'TypeList[...]' value has no attribute
'size'`) — both bridges expire and those probes are being promoted to
`type_error` fixtures by the current pass. The remaining rows still hold.
Additional hand checks the same day: the `read` convention is a hard error
(`'read' was removed; use 'imm'`), `@parameter` on parametric closures warns
(`deprecated; use '@__parameter'`) but still runs, `UnsafeMaybeUninit` is
removed outright (`MaybeUninit` carries the same `unsafe_*` vocabulary), and
`UnsafePointer` still warns-and-runs as a deprecated alias of `Pointer`.

| Probe | Question | Expected on both |
|---|---|---|
| `tuple_element_types_public_spelling.mojo` | Does the head keep Tuple's `*Ts` parameter and `element_types` member spellings? (Verified in source at `ae386d1b204`; accepted without warning at `a79fbdf59f2`.) | runs, prints `2` / `7` |
| `element_call_member_base.mojo` | Does the head dispatch the bare member-base element call `h.items[0](5)` like the confirmed identifier base? (Re-confirmed at `a79fbdf59f2`.) | runs, prints `15` |
| `element_call_multi_index.mojo` | Does the head dispatch the bare multi-index element call `g[1, 1](10)` through the variadic subscript? (Re-confirmed at `a79fbdf59f2`.) | runs, prints `40` |
| `pack_overload_string_regular_ambiguity.mojo` | Why is a call ambiguous between `g(a: Int, *rest)` and `g(a: Int, b: String, *rest)` when the same shape with `b: Int` is not? (Observed at `a79fbdf59f2`, 2026-09-19.) | **differs**: the pin rejects (`ambiguous call to 'g'`), Mojito prints `3` — `docs/roadmap.md` §1 |

## Parameter expressions (follow-on shapes)

The parameter-expression entry's own probes were answered and promoted
(`assets/ok/param_expr_*.mojo`, `assets/type_error/param_expr_*.mojo`; the
record is `docs/notes/param-expr-attributes.md`), and so was its follow-on
shape, the symbolic SIMD width (`assets/ok/simd_symbolic_width_hooks.mojo`,
2026-09-22).

## Tuple element mutability (re-run at every re-pin)

Found while restoring the public Tuple's structural surface, 2026-09-20.

| Probe | Question | Expected on both |
|---|---|---|
| `tuple_element_write.mojo` | Is `t[0] = 9` a write through `Tuple`'s compile-time-index hook? | **differs**: the pin prints `9`, Mojito rejects ("Tuple elements are immutable") — `docs/roadmap.md` §3, `tuple-element-write` |

## Pack-keyed template bodies (re-run at every re-pin)

A body keyed on a variadic pack is validated from its template, with the
element at a symbolic index opaque. Observed 2026-09-20 against
`Mojo 1.1.0.dev2026082605 (dd957314)`; both compilers agree on every row.

| Probe | Question | Expected on both |
|---|---|---|
| `pack_body_untaken_arm.mojo` | Is an untaken arm of a never-instantiated pack body (a struct's pack, a `def`'s own) checked? | reject (no such member on the element) |
| `pack_element_bound_source.mojo` | Which sources license a trait use of an element: the pack's bound, a method `where conforms_to(Ts.values, …)`, a method `where Ts.all_conforms_to[…]()`? | runs, prints `1` `two` three times |
| `pack_element_header_conformance_only.mojo` | Does a struct-header conditional conformance license the method that implements it? | reject (the method needs its own `where`) |
| `pack_element_where_disjunction.mojo` | Does a disjunctive `where` license an element use? | reject |
| `pack_constant_index_symbolic_pack.mojo` | What is `self.storage[0]` over an unbound pack? | reject (the element stays dependent, never `Int`) |
| `pack_runtime_index.mojo` | A runtime index into `*args: *Ts`. | reject |
| `pack_contains_marker.mojo` | Are both arms of `comptime if Self.Ts.contains[T]()` checked with `T` symbolic? | reject (the untaken arm's type error) |
| `pack_mixed_spread.mojo` | `Tuple[Int, *Self.Ts]`: a spread beside a fixed argument. | reject |
| `pack_forwarding_untaken_arm.mojo` | Is a body that forwards its pack (`inner(*a)`) checked from the template? | **differs**: the pin rejects the untaken arm, Mojito reaches no verdict on that body and prints `1` `two` — `docs/roadmap.md` §1 |
| `abort_ends_a_returning_body.mojo` | Does a trailing `abort(...)` end a value-returning body? | **differs**: the pin prints `1`, Mojito rejects ("does not return a value on every path") — `docs/roadmap.md` §3 |
| `borrowed_default_argument_destructor.mojo` | Is an evaluated default handed to a borrowing parameter destroyed when the call returns? | **differs**: the pin prints `drop dflt`, Mojito's VM and native backend never run the destructor — `docs/roadmap.md` §3 |
| `list_literal_default_argument.mojo` | Does `xs: List[Int] = [1, 2, 3]` declare? | **differs**: the pin prints `3`, Mojito rejects the default as an `Array[Int, 3]` — `docs/roadmap.md` §3 |
| `implicit_conversion_bound_on_declaration.mojo` | Is an `@implicit` conversion in a generic method that still clones per instance selected once, on the declaration? | **differs**: the pin prints `2` twice, Mojito selects again in the `Box[Int]` clone and reports an ambiguity. A method the template serves agrees (`assets/ok/implicit_conversion_bound_on_declaration.mojo`) — `docs/roadmap.md` 3.96 |
| `equatable_witness_of_another_type.mojo` | Which `__eq__` serves `==` through an `Equatable` bound when the struct's own takes another type? | **differs**: the pin takes `Equatable`'s fieldwise default and prints `True`, Mojito names the declared `__eq__` and stops in elaboration — `docs/roadmap.md` 3.97 |
| `consuming_conversion_copies_parameter.mojo` | May an `@implicit` conversion through a consuming constructor copy a place of a `Copyable` parameter type? | **differs**: the pin rejects the declaration ("value of type 'T' cannot be implicitly copied"), Mojito prints `7` twice — `docs/roadmap.md` 3.98 |

## Reflection-reading template bodies (re-run at every re-pin)

A body reading `reflect[T]` over a symbolic `T` is validated from its
template; a field type under a symbolic index is opaque until a
`conforms_to` arm proves a trait of it. Observed 2026-09-23 against
`Mojo 1.2.0.dev2026092105 (e9569894)`.

| Probe | Question | Expected on both |
|---|---|---|
| `reflection_body_untaken_arm.mojo` | Is an untaken arm of a never-instantiated body reading `reflect[T]` checked? | reject (the arm's type error) |
| `reflection_field_type_proof.mojo` | What does `comptime if conforms_to(types[i], Defaultable & Writable):` license on the field type? | reject (`Sized` was not proved); accepted once the `len` line goes |
| `reflection_proof_is_positional.mojo` | Does a proof on another index, after the use, or in another loop license a use? | reject, three times |
| `template_fallback_reflection.mojo` | Does a validated reflection body still check per instance? | runs, prints `2` `0` |
| `comptime_for_body_scope.mojo` | Is each unrolled iteration of a `comptime for` body its own scope? | **differs**: the pin prints `0` `1`, Mojito rejects ("'v' is already declared in this scope") — `docs/roadmap.md` §3 |
| `mut_self_hash_witness.mojo` | Does a `mut self` `__hash__` witness a read-`self` requirement? | **differs**: the pin prints `True`, Mojito rejects the conformance ("missing required operation") — `docs/roadmap.md` 3.48 |
| `imported_alias_in_generic_method.mojo` | Does an imported alias of a struct application resolve in the signature of a generic struct's method that still clones per instance? | **differs**: the pin prints `True`, Mojito reports "unknown type '__module$hasher$default_hasher'" for the clone — `docs/roadmap.md` 3.49 |
| `setter_without_getter.mojo` | Is a subscript store accepted on a struct that declares `__setitem__` but no `__getitem__`? | **differs**: the pin rejects the store ("'Sink' has '__setitem__' but no '__getitem__' method"), Mojito prints `3` — `docs/roadmap.md` 3.16 |
| `bound_call_result_converted_at_binding.mojo` | Does a generic body bind a bound method's call result through an `@implicit` conversion at an annotated `var`? | **differs**: the pin prints `2 a`, Mojito rejects the generic body ("register r1 has no checked type") — `docs/roadmap.md` 3.24 |
| `indexed_copyable_element_transfer.mojo` | May an implicitly copyable, non-trivial indexed element be transferred (`x[0]^`)? | **differs**: the pin rejects ("expression does not designate a value with an origin"), Mojito prints `1` — `docs/roadmap.md` 3.12 |
| `reference_binding_transfer.mojo` | Which phase rejects a `^` transfer of a `ref` binding? | **differs**: the pin rejects ("expression does not designate a value with an origin"), Mojito rejects at MIR verification with an internal message — `docs/roadmap.md` 3.100 |
| `variadic_var_collector_element_augmented_operand.mojo` | Can an element of an owned `String` collector be the right operand of `+=`? | **differs**: the pin prints `xy`, Mojito stops ("register r12 has no checked type") — `docs/roadmap.md` 3.5 |
| `collector_runtime_index_native.mojo` | Does a collector indexed by a runtime value compile natively? | VM prints `6`; `--backend pliron` rejects ("unsupported runtime index into pack storage") — `docs/roadmap.md` 2.3 |
| `comprehension_binder_method_call.mojo` | Does a method called on a borrowed comprehension binder run? | **differs**: the pin prints `2`, Mojito stops at run time ("call passed 0 args to 1-parameter function 'P.get'") — `docs/roadmap.md` 3.37 |
| `generic_method_transfer_in_try.mojo` | Does a generic method move its `var` parameter into a field's `append` inside `try`/`finally`, as under a `with`? | **differs**: the pin prints `finally` `2`, Mojito rejects the template ("use of uninitialized value 'value'") — `docs/roadmap.md` 3.21 |
| `value_struct_generic_constructor_overload.mojo` | Does a generic constructor declared beside another one run on a struct with a value parameter? | **differs**: the pin prints `5` `5` `1`, Mojito stops at run time ("vm backend does not support the built-in or callee 'P.__init__$ov$T$Writable' yet") — `docs/roadmap.md` 3.103 |
| `mut_capture_transfer_refilled.mojo` | May a `mut` capture be transferred away when the body writes it back? | **differs**: the pin rejects ("cannot consume indirect references to values"), Mojito prints `ab` — `docs/roadmap.md` 3.104 |
| `trivial_struct_mut_parameter_transfer.mojo` | Does `^` copy a `TrivialRegisterPassable` struct out of a `mut` parameter? | **differs**: the pin prints `3 3`, Mojito rejects ("'v' is uninitialized at return from this function") — `docs/roadmap.md` 3.105 |
| `pointer_copied_into_struct_keeps_loan.mojo` | Does a pointer handed to a struct's constructor keep its pointee alive? | **differs**: the pin prints `2` `2` `2` `3`, Mojito stops at run time ("use after Pointer deallocation") — `docs/roadmap.md` 3.106 |
| `loan_carrying_temporary_owned_argument.mojo` | Is a heap-owning temporary that carries a loan destroyed once when an owning parameter takes it? | **differs**: the pin prints `1` `1`, Mojito stops at run time ("use after Pointer deallocation") — `docs/roadmap.md` 3.107 |
| `immutable_origin_struct_argument_exclusivity.mojo` | May a `Span` over an immutable origin be handed to a method of a receiver naming the same origin? | **differs**: the pin prints `1 2`, Mojito rejects ("aliasing values passed mutably") — `docs/roadmap.md` 3.108 |
| `len_of_dereferenced_pointer_field.mojo` | Does `len` run on a list reached through a pointer field? | **differs**: the pin prints `3`, Mojito stops at run time ("methods on ref") — `docs/roadmap.md` 3.109 |
| `per_call_clone_copied_loan.mojo` | Does a method with a parameter of its own keep the loan of a value it stores a copy of? | **differs**: the pin prints `3 1 8`, Mojito stops at run time ("checked nominal subscript receiver is None") — `docs/roadmap.md` 3.110 |
| `method_calls_later_generic_def.mojo` | May a method call a generic `def` declared after its struct? | **differs**: the pin prints `1`, Mojito stops with "Undefined variable 'tally'" — `docs/roadmap.md` 3.111. |

## Re-probes of enforced claims

These rejections were enforced by the slice-A alignment sweep and confirmed
against the `ae386d1b204` build (2026-08-15); re-confirmed against
`a79fbdf59f2` (2026-08-26). Confirm they still hold at each re-pin.

| Program | Expected on both | Enforced by |
|---|---|---|
| `../fixtures/unified_capture_lists.mojo` | reject (`unified` removed) | parser error |
| `../fixtures/setter_overload_extension.mojo` | reject (competing `__setitem__` pair) | declaration-time checker error |
| `../fixtures/captured_nested_origin_specialization.mojo` | reject (capturing nested fn as specialized value) | checker error at value materialization |
| `../../assets/type_error/capturing_closure_plain_def_param.mojo` | reject (capturing closure into unqualified `def(...)`) | value-coercion checker error |

Bridges to re-check by hand each re-pin (no standalone probe): `UnsafePointer`
remains a deprecated alias of `Pointer` upstream (Mojito keeps accepting it as
a bridge), and the `subtree-origin-cast` mojito-only case documents Mojito's
`._subtree` cast acceptance against upstream's pass-manager failure.

## Compile-time region ownership (2026-10-02)

`comptime_region_*.mojo` are the probes behind
[`docs/notes/comptime-region-ownership.md`](../../docs/notes/comptime-region-ownership.md),
which tabulates the pin's verdict and destructor order for each, beside the
runtime-shaped twin (`comptime if` spelled `if`, `comptime for` spelled `for`)
and the `--comptime-regions keep` experiment. The rule: a compile-time region
is decided as the runtime region of the same shape, its condition opaque and
its trip count unknown. Re-run after every re-pin; the five probes Mojito
runs where the pin rejects (`c2`, `c4`, `c5`, `l1`, `l3`) are also
`assets/extensions/ownership_ok/` fixtures on the roadmap ledger.

| Probe | Question | Answer |
| --- | --- | --- |
| `comptime_region_c1..c10_*.mojo` | How do the arms of a `comptime if` join: last use, early return, raise, a reference-bearing value, an invalid untaken arm, an invalid unused declaration? | As an `if`'s arms: the pin rejects a move in either arm followed by a use after the join (`c1`, `c2`), an untaken arm's use of a moved value (`c4`), an unused declaration that moves (`c5`), and a use after a join one arm returns from while the other consumes (`c6`); a value one arm consumes is destroyed at the other arm's entry (`c3`, `c7`); a reference keeps its referent to its last read (`c8`). |
| `comptime_region_l1..l7_*.mojo` | How is a `comptime for` body decided: zero, one, several iterations, loop-carried ownership, compile-time `break` and `continue`? | As a loop body with the trip count unknown: a move without a refill is rejected at zero and at one iteration (`l1`, `l2`, `l3`); a refilled move is fine (`l5`); a per-iteration value dies in its iteration, at a `continue`, or at the entry of a breaking arm (`l4`, `l6`). Mojito rejects compile-time `break`/`continue` in the symbolic validation (`l6`, `l7`). |
| `comptime_region_l8_*.mojo`, `comptime_region_l9_*.mojo` | How is a heterogeneous owned pack destroyed? | At the body's entry when unused, in reverse element order, each element by its own destructor; an empty pack destroys nothing. Mojito matches. |
| `raise_path_live_value_leaks.mojo` | Is a value live after a conditional region destroyed when the region raises out of the function? | **differs**: the pin prints `del b3`..`del b7`; Mojito destroys only `b4`, which is dead at the raise. Roadmap 3.1. |
