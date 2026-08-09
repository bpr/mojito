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

## Open questions (audit `ae386d1b204`)

| Probe | Question | Mojito today |
|---|---|---|
| `def_typed_local_annotation.mojo` | Is a bare `def(...)` local `var` annotation a callable-value position in current Mojo, or a trait like field/element positions? | runs, prints `42` |
| `generic_function_value_materialization.mojo` | Does current Mojo contextually materialize a generic function into a `def(Int) -> Int` value? (Moot if the previous probe rejects.) | runs, prints `42` |
| `simd_size_deprecated_alias.mojo` | Does the head still accept deprecated `SIMDSize`? (Deprecation-bridge tracking; verified in source at `ae386d1b204`.) | runs, prints `4` |

## Re-probes of enforced claims (evidence from the `609afcd0735` pin)

These rejections were enforced by the slice-A alignment sweep on the strength
of the previous audit; confirm they still hold on the current head.

| Program | Expected on both | Enforced by |
|---|---|---|
| `../fixtures/unified_capture_lists.mojo` | reject (`unified` removed) | parser error |
| `../fixtures/setter_overload_extension.mojo` | reject (competing `__setitem__` pair) | declaration-time checker error |
| `../fixtures/captured_nested_origin_specialization.mojo` | reject (capturing nested fn as specialized value) | checker error at value materialization |
| `../../assets/type_error/capturing_closure_plain_def_param.mojo` | reject (capturing closure into unqualified `def(...)`) | value-coercion checker error |
| `../../assets/type_error/callable_element_call_parses_as_parameter_application.mojo` | **Mojo runs (prints `6`), Mojito rejects** — recorded subset gap | parenthesization-hint diagnostic |
