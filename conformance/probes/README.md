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
| `element_store_from_same_list.mojo` | Is an element stored from another element of the same `List` field accepted whatever the element type? | **differs**: the pin prints `5` `y`, Mojito rejects the `String` instance ("'self' is borrowed mutably and also used at the same call") — `docs/roadmap.md` 3.6 |
| `comptime_for_body_scope.mojo` | Is each unrolled iteration of a `comptime for` body its own scope? | **differs**: the pin prints `0` `1`, Mojito rejects ("'v' is already declared in this scope") — `docs/roadmap.md` §3 |
| `overloaded_hash_through_hash.mojo` | Does `hash` reach a struct that overloads `__hash__` on the hasher's type? | **differs**: the pin prints `True`, Mojito stops at run time ("vm: unknown method 'Twin.__hash__'") — `docs/roadmap.md` 3.5 |
| `mut_self_hash_witness.mojo` | Does a `mut self` `__hash__` witness a read-`self` requirement? | **differs**: the pin prints `True`, Mojito rejects the conformance ("missing required operation") — `docs/roadmap.md` 3.51 |
| `imported_alias_in_generic_method.mojo` | Does an imported alias of a struct application resolve in a generic struct's method signature? | **differs**: the pin prints `True`, Mojito reports "unknown type '__module$hasher$default_hasher'" — `docs/roadmap.md` 3.52 |
| `setter_without_getter.mojo` | Is a subscript store accepted on a struct that declares `__setitem__` but no `__getitem__`? | **differs**: the pin rejects the store ("'Sink' has '__setitem__' but no '__getitem__' method"), Mojito prints `3` — `docs/roadmap.md` 3.53 |
| `bound_call_result_converted_at_binding.mojo` | Does a generic body bind a bound method's call result through an `@implicit` conversion at an annotated `var`? | **differs**: the pin prints `2 a`, Mojito rejects the generic body ("register r1 has no checked type") — `docs/roadmap.md` 3.23 |

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
