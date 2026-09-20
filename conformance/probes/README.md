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
record is `docs/notes/param-expr-attributes.md`). One shape stays open.

| Probe | Question | Expected on both |
|---|---|---|
| `param_expr_simd_hooks.mojo` | Do symbolic SIMD widths canonicalize (`SIMD[dt, n + 1]` as `SIMD[dt, 1 + n]`, `SIMD[DType.int64, Self.length]`)? (Observed at `a79fbdf59f2`, 2026-09-20.) | **differs**: the pin runs it, Mojito's `Ty::Simd` needs a concrete width — `docs/roadmap.md` §1, the `DType`/vector validation entry |

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
