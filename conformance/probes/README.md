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

| Probe | Question | Expected on both |
|---|---|---|
| `simd_size_deprecated_alias.mojo` | Does the head still accept deprecated `SIMDSize`? (`@deprecated` in source at `ae386d1b204`.) | runs, prints `4` (Mojo warns) |
| `typelist_size_deprecated_alias.mojo` | Does the head still accept deprecated `TypeList.size`? (`size = Self.length` alias in source at `ae386d1b204`.) | runs, prints `2` (Mojo warns) |
| `tuple_element_types_public_spelling.mojo` | Does the head keep Tuple's `*Ts` parameter and `element_types` member spellings? (Verified in source at `ae386d1b204`.) | runs, prints `2` / `7` |
| `element_call_member_base.mojo` | Does the head dispatch the bare member-base element call `h.items[0](5)` like the confirmed identifier base? | runs, prints `15` |
| `element_call_multi_index.mojo` | Does the head dispatch the bare multi-index element call `g[1, 1](10)` through the variadic subscript? | runs, prints `40` |

## Re-probes of enforced claims

These rejections were enforced by the slice-A alignment sweep and confirmed
against the `ae386d1b204` build (2026-08-15); confirm they still hold at each
re-pin.

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
