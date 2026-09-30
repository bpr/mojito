# Pliron A1 — Shadow `mojito.core` Proof (record)

Record of the Stage A1 vertical slice from
[`docs/pliron-backend-pivot-plan.md`](../pliron-backend-pivot-plan.md)
§Smallest falsifiable proof.

## Status and decision

**`PASS-SLICE`, and `INCOMPLETE` for the pivot decision.** Recorded
2026-09-27; the decision corpus run 2026-09-29.

- All seven proof items pass on the gate fixture, on the VM and natively.
- F1 to F4 pass. F5 (overhead) is **not measured**.
- The pivot is neither approved nor rejected. A2 is not started.
- All ten focused inputs convert, and 887 of the 893 inputs of the decision
  corpus (§Coverage). The six that do not are named there; none is a form
  the inventory lacks.
- The scheduled measurement lane and the pin rehearsal have not run
  (§Remaining boundaries).

The MIR waist, the v1 `.mir` schema, the VM, and the native backend's MIR
input are unchanged. A1 is optional, default off, and removable.

## Verdict table

| Criterion | Result | Evidence |
|---|---|---|
| F1: represent and verify the slice cleanly | PASS | `a1_representation_contract`, `a1_malformed_ops_are_diagnostics` (185 cases, 0 accepted, 0 panics), `a1_same_module_executes` |
| F2: locations, lifecycle, artifact fidelity | PASS | `a1_locations_per_op`, `a1_location_mutations_name_their_op`, `a1_lifecycle_edges`, `a1_canonical_first_print`, `a1_v1_exec_roundtrip`, `a1_dead_scalar_preserves_events` |
| F3: conversion totality | PASS | `a1_inventory_is_closed`, `a1_conversion_is_total`, `a1_focused_inputs_convert` |
| F4: default-lane isolation | PASS | both default graphs byte-identical to the captured ones, no `pliron`, `pliron-llvm`, or `llvm-sys` package, default build succeeds with LLVM discovery removed |
| F5: disproportionate overhead | NOT MEASURED | harness ready; 10 of 10 focused inputs convert; neither lane is scheduled |
| Full canonical-artifact compatibility | 887 / 893 | `scripts/cover-pliron-a1` over `corpus-inputs.tsv`; the six residues in §Coverage |
| Pin-maintenance rehearsal | NOT MEASURED | scheduled work, one working day |

Core text is 2.6 to 3.4 times the v1 text of the same module. That is over
the 2.0 line that makes a design review mandatory (finding 6). It is not a
substitute for the time and memory thresholds.

## Pins and environment

| | |
|---|---|
| Repository | `d846604a59d8d93353cee9f2d22a6cfed767265f`, plus this task's uncommitted changes |
| Rust | 1.96.1 (31fca3adb 2026-06-26) |
| Pliron | git `477e6b0edb18b29df4cf7b90f0f468dc8a872f22` (`pliron`, `pliron-llvm`) |
| LLVM | 23.1.0 at `/opt/llvm-23`, `llvm-sys` 231 |
| Host | Linux 7.1.1, Intel Core i7-10875H |
| Gate | `assets/extensions/ok/pliron_a1_gate.mojo`, sha256 `b337ee1c…7b50f032` |

## The gate and its prerequisite

The gate is a kept-extension fixture (direct `ref` field), so it sits under
`assets/extensions/ok/` and adds no parity-manifest row.

Slice zero reproduced the planning result: the VM ran the fixture, and the
native backend failed IR verification with `Op has different operand types`.

**Cause.** MIR loads a `ref`-typed field with `place.load` and feeds the
register to `binary` with no `ref.read` between. The VM's `LoadPlace` reads
through a reference-typed place, so its register holds the referent. The
native backend keeps the handle in the register, which `print` and call
arguments need, and `lower_binop` passed that pointer to the add.

**Fix.** The native operand path reads through a handle to scalar storage
(`FnLowering::operand_value`, `referent_scalar_ty` in
`crates/mojito-pliron/src/lower/emit.rs`). It covers binary operators, true
division, and unary operators. A handle to a pointer or an aggregate stays a
handle. No MIR contract, ABI, or dependency edge changed.

`a1_reference_field_arithmetic_matches_vm` pins the gate and an operand
matrix (immutable and mutable references over `Int`, `Float64`, `Bool`, with
owned scalars as the control) at O0, release, and under lifecycle tracing.
This fix is independent of the pivot and stays whatever the verdict.

## Accepted contracts

These are the decisions as built. Changing one is a reviewed change.

### Boundary

- `crates/mojito-pliron/src/a1/`, behind crate feature `a1-core` and root
  feature `pliron-a1`. No new dependency edge.
- The wire namespace is `mojito_core`. Pliron identifiers admit no dot, and
  an operation id has one separator.
- Mojito symbols (`Proof.read$i1;`) are typed string attributes. A function's
  Pliron symbol is `f<index>`.
- Input is verified, drop-elaborated MIR after `native::mono::specialize`.
  Specialization is VM-transparent for the gate: stdout and the ordered
  lifecycle log are identical.

### Inventory

53 operations, all defined through one macro over the registry
(`inventory::CoreOpKind`). Each entry owns its name, effect class, legal
stages, MIR forms, export rule, and text rule.

| Stage | Operations |
|---|---|
| Both | `func` `slot` `const` `materialize` `binary` `simd_make` `unary` `simd_convert` `use` `store` `project` `load` `ref_make` `ref_store` `ref_read` `copy` `move` `keep_alive` `index` `multi_set` `pointer_storage` `iter_init` `iter_next` `call` `closure_make` `field_get` `size_of` `ref_write` `multi_index` `slice` `simd_shuffle` `pack_has_next` `pack_next` `uninit_storage` `variant_make` `variant_test` `variant_get` `variant_set` `variant_deinit_with` `drop` `consume` `loans` `invalidate` `br` `cond_br` `return` `raise` |
| Bridge only | `try_bridge` `region_exit` `escape` |
| Executable core only | `invoke` `outcome` `resume` |

Differences from the plan's initial list, all from the census:

- Added `simd_make`, and the `UInt`, pointer, and SIMD types. Printing an
  `Int` reaches three `std._intrinsics` digit helpers that build `UInt8`
  scalars and write through a pointer.
- Omitted `ref_write`. No focused input's closure holds one. The importer
  rejects its MIR form by name.

The gate's closure needs 26 of the 37. The other eleven come from the
compile benchmarks:

| Operation | MIR forms | Effect |
|---|---|---|
| `unary` | `unary` | pure |
| `simd_convert` | `simd.cast`, `simd.bits` | pure, never removed |
| `ref_read` | `ref.read` | effectful |
| `copy` | `value.copy` | effectful |
| `move` | `place.move` | effectful |
| `keep_alive` | `lifetime.keep_alive` | effectful |
| `index` | `index.get` | effectful, raising |
| `multi_set` | `index.multi_set` | effectful, raising |
| `pointer_storage` | `pointer.take`, `pointer.destroy` | effectful |
| `iter_init` | `iter.init` | effectful |
| `iter_next` | `iter.try_next` | effectful, two value results |

Two existing operations import a second form: `drop` takes `drop.place`,
and `return` takes `return.cleanup` with its cleanup slots as operands.

The decision corpus added sixteen operations and four second forms
(`call` takes `call.indirect`, `consume` takes `consume.place`, and the
variant and uninitialized-storage operations take one form per access):

| Operation | MIR forms | Effect |
|---|---|---|
| `closure_make` | `closure.make` | effectful; one `Move` lifecycle event per moved capture |
| `field_get` | `field.get` | pure |
| `size_of` | `layout.size_of` | pure |
| `ref_write` | `ref.write` | effectful |
| `multi_index` | `index.multi` | effectful, raising |
| `slice` | `slice.get` | effectful, raising |
| `simd_shuffle` | `simd.shuffle` | pure |
| `pack_has_next`, `pack_next` | `iter.has_next`, `iter.next` | effectful |
| `uninit_storage` | `uninit.make`, `uninit.take`, `uninit.destroy` | effectful |
| `variant_make`, `variant_test`, `variant_get`, `variant_set`, `variant_deinit_with` | `variant.make`; `variant.is`; `variant.get`, `variant.take`; `variant.set`, `variant.replace`, `variant.set_init_with`; `variant.deinit_with` | effectful, but `variant_test` pure |
| `escape` | `escape` | terminator, bridge only |

Of the 63 MIR forms, 59 import and 4 are rejected by name
(`target/pliron-a1/form_decisions.tsv`): `drop.reg` is reserved and
unemitted, `unsupported` is lowering's own failure marker, and
`tuple.make` and `type.construct` occur in no corpus input, so no positive
case exists for them. Neither match has a wildcard arm.

### Types, declarations, and calls

The benchmarks also needed facts the gate never carried:

- Types: compiler-private tuple storage, the runtime pack, a type
  parameter named by identity with its bounds, and a pointer origin that
  names an origin binder or the receiver's own place. Specialization
  leaves origins symbolic. The corpus added `float64`, `float_literal`,
  `dtype`, `variant`, union origins, and `func`: the full checked function
  type (environment, parameters with conventions and reference contracts,
  variadic collectors, markers, error, reference result, and transfer
  effects). `transfers` are part of the `func` type's identity, as the v1
  text prints them, where checked types ignore them: two checked-equal
  function types can be two handles.
- Constants: `Int`, the integer literal, `Float64` as its bits, the
  floating literal in its own exact spelling, `Bool`, string, `DType`,
  and a function symbol. A negative machine integer prints through the
  `Signed` newtype: Pliron's own integer parser reads digits alone.
- Declarations: parameter defaults (a literal under its converting
  constructors, floating literals included), both variadic collectors,
  and the positional-only and keyword-only markers.
- Calls: the compile-time arguments of a call, each with its name, its
  reifying register as an operand, its forwarded binder, and its value
  expression as a parameter node; the captured owner storage the call
  reads or writes; a method's reference result and result adapter; and
  an indirect call's callee as the receiver operand, with its
  instantiated contract and retained generic arguments. A call may
  record no place table.
- Callees: a direct call names a function, a struct, a builtin the VM
  answers itself, or the VM's dynamic trait dispatch; a method call
  without a checker-selected symbol names a method of a compiler-private
  value the VM implements. Both name lists live beside the dispatch that
  answers them (`mojito_vm::builtins`) and are re-exported as
  `verify::{BUILTIN_CALLEES, BUILTIN_METHODS}`.
- A projection may designate the referent of its last step's reference,
  as MIR does when it reads through a `ref` field. A field step's type is
  compared with the declaration's modulo every origin, since a shared
  instance spells them two ways.
- `ref_make` and `ref_store` yield or take a reference capability: a
  reference, or a pointer whose origin names a place, a binder, or the
  receiver, as MIR's own verifier admits.
- A SIMD construction or conversion of one lane may land in the scalar
  alias the checker spells it as (`Int`, `Bool`, `Float64`); the
  operation records its lane type and width itself.

### Values and registers

- A register defined once and read only later in its own block is a direct
  SSA value.
- Any other register gets a slot tagged `Register`, with transport loads and
  stores. Export checks each transport against its producer and coalesces it.
- Every value result carries its MIR register as an attribute. That is
  compatibility metadata, not a name.
- Every effectful operation consumes the current effect token and yields the
  next. The function verifier checks one chain per block.

### Outcomes

Normalization rebuilds each function as a flat graph:

- A raising call or subscript becomes `invoke` with a normal and an error
  successor. `invoke` keeps the facts of the operation it replaced, and
  denormalization restores that operation.
- A `try` gets an error-edge block and a normal-edge block. Each runs the
  cleanup list in order, as the VM's `exec_try` does on both edges.
- A `finally` has one entry taking a pending outcome. Every way in records
  `outcome` first, and `resume` dispatches after the body.
- An error leaving the function lands on a block that raises it.
- A `return` or `escape` crossing out of a try region runs the drops of
  the tries it leaves inline, innermost first (`ExitDrop(n)` roles), then
  its terminator. A `finally` on the way makes the exit a pending outcome
  (`outcome` kind `Exit(site)`) entering that finally; `resume` continues
  each site after the finally's own normal and error successors
  (`ResumeAttr`), into a continuation block holding the next drops and the
  way out. A return carrying cleanup across a finally, and an exit from
  inside a finally body, are refused by name.

Export first rebuilds the structured regions from the current operations.
The layout attribute records, per block, its region, MIR block, and segment.
It holds no instruction. A graph the layout does not describe is rejected.

### Lifecycle

Each normalized function carries a contract: its ordered initialization,
move, drop, and consume events, each with its block, its position in the
effect chain, and the states its owner may be in before it. It is derived
once, at normalization, and recomputed and compared at every verification.

A drop of an already-empty slot stays legal. The gate has several.

### Locations

- Every operation carries typed provenance: the input's source record
  exactly, or its recorded absence, or the identities it was derived from.
- Every operation and block carries an explicit Pliron location naming its
  stable identity, so a parse restores it.
- In the gate: 143 operations with a source record, 37 derived, 110 absent.

### Canonical text

`C(module)` is the print of the module after a parse into a fresh context.
Pliron names values by arena index, so text depends on allocation history.
The parser allocates in the order of the text's structure, which no name and
no history affects. `C(m) == C(parse(C(m)))` holds from the first parse.

The wrapper's first line is `mojito-a1-core 0`. Production `exec` refuses
it. v1 text exported from unoptimized core is byte-identical to the input.

### Parameter attributes

`a1::params` re-homes `ParamExpr` as context-owned payloads with
context-local child keys, exhaustive over `ParamKind`. Construction, folding,
and replacement stay with `ParamContext`.

- A callable occurrence keeps its transfer effects in the payload's identity,
  so two occurrences that are equal types are two nodes.
- A closed value argument of a nominal type is a constant node. An open one
  is refused.
- The constant subset is `Int`, `UInt`, integer literal, `Bool`, string,
  `DType`, and type. Other constants are refused by name.

## Findings

1. **The slice is representable without an opaque payload.** No operation
   carries a MIR instruction or body. Export was run after the importer's
   program was dropped.
2. **The pinned Pliron exposes no enumeration of a dialect's operations.**
   `Dialect::ops` is crate-private. Inventory closure is checked from the
   source of `ops.rs` and from the text of both stages instead.
3. **Pliron's string printer and parser disagree on escapes.** The printer
   uses Rust's `{:?}`, and the parser accepts only `\\` and `\"`. A1 uses its
   own `Text` type. The same holds for a named location, whose name must hold
   no quote.
4. **First-print stability needs a parse.** Erasing given names is not
   enough, as the plan expected. The fresh-context copy costs a print and a
   parse, and both are charged to the shadow.
5. **Text dominates the boundary cost.** In the debug diagnostic below,
   canonicalization and the fresh parse are 780 ms of 870 ms.
6. **Core text is large.** 2.6 to 3.4 times v1. Identity and provenance
   attributes repeat the function symbol on every operation.
7. **Export needs a constructor that does not fold.** `ParamContext::op`
   folds the closed atom `8 // 2` to `4`, and cancels a term holding a
   partial atom, where `replace` and `infix` keep both. Export rebuilds an
   operator through `ParamContext::rebuild`, which keeps both shapes, so
   every canonical node round-trips; a stored operator that is not
   canonical is still refused rather than given another identity.
8. **The VM runs `finally` on a trap.** Normalized core has no trap edge, as
   the native backend has none. Both backends execute exported MIR here, so
   the adapter cannot show the difference. An A2 interpreter must decide it.
9. **A lifecycle mutation is often caught at a later event.** Hoisting a
   consume above a raising call is reported at the error-edge cleanup, whose
   owner state changed. The diagnostic names the first event that deviates.
10. **The form census understated the distance.** The fifteen uncovered
    forms hid four more refusal classes: compile-time call arguments,
    declaration defaults and markers, storage and parameter types, and
    builtin callees. `import::refusals` now counts every class, and the
    harness prints it for a refused input.
11. **Specialized MIR did not verify for `stdlib_heavy`.** Monomorphization
    built a rewritten dunder call (`__eq__`) with no place table, and left
    a subscript contract's parameter types unsubstituted. Both are fixed in
    `crates/mojito-native/src/native/mono/`. The native backend never ran
    `mir::verify` on its specialized input, so nothing had reported them.
12. **Specialization was not VM-transparent beyond the gate.** For `tuple`,
    `tstring`, and `stdlib_heavy` the specialized program did not run on
    the VM as the original did: monomorphization dropped the nominal
    `String`'s `__copyinit__` and `write_to`, which the native backend
    bridges and the VM runs. Both are now kept, native reachability still
    skips them, and the VM's display dispatch accepts a `write_to` whose
    writer specialization bound to the builtin string writer.
13. **The pinned Pliron panics on a block nothing reaches.** Its dominance
    check indexes a dominator map that holds only the blocks reachable
    from the region's entry, and a `try` whose body cannot raise leaves
    its whole error path unreachable after normalization. `verify_tree`
    now runs Pliron's verification of everything but dominance, and
    `verify_dominance` judges every use itself, skipping dead blocks. To
    raise upstream at the pin rehearsal.
14. **The census was partial.** The parallel walker behind
    `import::refusals` mirrored seven of the importer's rules and kept a
    second copy of the callee policy; 105 of 517 refused inputs had no
    counted refusal. The importer now runs in collect mode instead, and
    each finished operation is verified in place, so the census counts
    verifier and legality rules too.
15. **Three monomorphization defects hid behind the shadow.** An
    arity-specialized variadic instance kept the ABI-only `RuntimePack` in
    its slots, a retargeted method call kept the compile-time parameter
    declarations its instance had shed, and the specialized program was
    never verified by the native backend. Both are fixed in
    `crates/mojito-native/src/native/mono/`; three inputs still do not
    verify after specialization (§Coverage).
16. **`#[format]` prints a variant's name before its own literal.** A
    tuple variant with a custom format holding a literal word printed the
    word twice (`Taketake`); the default tuple format is the one to use.

## Coverage

One shadow run per focused input, debug build, coverage only.

| Input | Converted | Functions | Operations | v1 bytes | Core bytes | Ratio |
|---|---|---|---|---|---|---|
| Gate | yes | 13 | 360 | 69,504 | 199,537 | 2.87 |
| `add` | yes | 6 | 213 | 39,172 | 119,146 | 3.04 |
| `empty` | yes | 2 | 67 | 9,716 | 32,658 | 3.36 |
| `hello` | yes | 5 | 201 | 36,411 | 113,377 | 3.11 |
| `generic` | yes | 29 | 845 | 186,488 | 506,954 | 2.72 |
| `tuple` | yes | 48 | 1,484 | 334,630 | 948,453 | 2.83 |
| `tstring` | yes | 42 | 1,439 | 322,533 | 936,365 | 2.90 |
| `stdlib_heavy` | yes | 172 | 6,024 | 2,092,497 | 6,558,137 | 3.13 |
| Gate ×16 | yes | 133 | 2,775 | 578,367 | 1,529,771 | 2.64 |
| Gate ×64 | yes | 517 | 10,503 | 2,212,971 | 5,827,648 | 2.63 |

For each of the seven compile benchmarks, `a1_focused_inputs_convert`
pins that the specialized closure passes `mir::verify`, that the importer
has no refusal, that the exported v1 text is the specialized text byte for
byte, and that the exported program runs on the VM as the specialized one
does, and the specialized one as the original.

The gate count here is 13 functions because the harness also enters
`__toplevel__`. The tests enter `main` alone and see 12.

### The decision corpus

`scripts/cover-pliron-a1` over the frozen 893 inputs
(`target/pliron-a1/corpus-inputs.tsv`), release build, one process per
input, coverage only. Run 2026-09-29 at each stage of the coverage task:

| Inventory state | Convert | Refused |
|---|---|---|
| Ten focused inputs (2026-09-28) | 376 | 517, of which 105 with no counted refusal |
| Defects fixed; floats, `DType`, function types, closures, indirect calls, reference results | 841 | 52 |
| The plain forms and variants | 887 with exits crossing a try refused | 6 plus those |
| Exits crossing a try, with and without `finally` | **887** | **6** |

The six that do not convert, none a form the inventory lacks:

| Input | Why |
|---|---|
| `assets/extensions/ok/container_owning_family_apis.mojo` | specialized MIR does not verify: an indirect-call target does not match its callable contract |
| `assets/extensions/ok/reference_list_write_through_method.mojo` | specialized MIR does not verify: a store of `ref Int` into `Int` storage |
| `assets/ok/capturing_lambda_array_display.mojo` | specialized MIR does not verify: a subscript's reference-result referent differs from the declaration |
| `assets/ok/tuple_hashable_dict_key.mojo` | a `Tuple` type argument spelled with and without its element types across one instance, so a field projection names no declared field |
| `assets/ok/comptime_shadowed_locals.mojo` | the front end refuses it under `Compiler::default()` (`expected Ts[i], found Ts[40]`) |
| `assets/ok/pliron_finally_overrides.mojo` | a `return` inside a `finally` body, refused by name |

Still refused by name, because no corpus input reaches them: a subscript
store through a slice, a return carrying cleanup across a finally, a
call with unresolved parameters, and the four rejected forms.

Beyond conversion, the exported program of some inputs does not run on
the VM as the specialized one does not either: the VM's slice bounds
construct the nominal `Optional`, whose template declaration
specialization drops. That is a VM-transparency defect of
monomorphization, filed on the roadmap, not a shadow defect: the exported
text is the specialized text byte for byte.

## Measurements

### Diagnostic only

One process per mode, **debug build, uncontrolled machine**. Not a verdict.

| Gate, `--phase compile` | Wall | Peak |
|---|---|---|
| baseline | 8.23 s | 295,916 KiB |
| shadow | 8.41 s | 295,636 KiB |
| shadow-opt | 8.44 s | 295,948 KiB |

Exclusive phase times of the shadow-opt run, in milliseconds:

| front end | specialize | construct | verify bridge | normalize | verify core | DCE | verify after DCE | canonicalize | parse | export | verify MIR |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 7,521 | 4.3 | 9.4 | 14.5 | 11.7 | 15.9 | 0.9 | 15.8 | 409.7 | 371.0 | 23.7 | 0.3 |

The dead-scalar test removes 3 of 302 operations and 1,340 of 169,661 bytes
in 3 sweeps.

### Pending

| Dataset / input | Count covered / total | Baseline compile median / p90 | Shadow compile median / p90 | Ratio ± 3 MAD | Construct ms | Verify ms | Canonicalize + parse ms | Export ms | DCE ms | Baseline / shadow peak KiB | Peak ratio bound | v1 / core bytes | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Gate | 1 / 1 | pending | pending | pending | pending | pending | pending | pending | pending | pending | pending | 69,504 / 199,537 | pending |
| Each of seven compile benchmarks | 7 / 7 | pending | pending | pending | pending | pending | pending | pending | pending | pending | pending | see §Coverage | pending |
| Gate ×16 / ×64 | 2 / 2 | pending | pending | pending | pending | pending | pending | pending | pending | pending | pending | see §Coverage | pending |
| Full decision corpus | not counted / 848 | pending | pending | pending | pending | pending | pending | pending | pending | pending | pending | pending | pending |
| Full canonical artifact compatibility | not counted | pending | pending | pending | pending | pending | pending | pending | pending | pending | pending | pending | pending |

## Probes and commands

Environment for every feature-gated command:

```sh
export LLVM_SYS_231_PREFIX=/opt/llvm-23
export PATH="$LLVM_SYS_231_PREFIX/bin:$PATH"
```

Named checks, each run as
`cargo nextest run -j 2 --features pliron-a1 -E 'test(=NAME)'`:

| Check | What it pins |
|---|---|
| `a1_specialized_closure_census` | specialization is VM-transparent, and writes the census |
| `a1_representation_contract` | bridge round trip, v1 byte equality, VM parity |
| `a1_malformed_ops_are_diagnostics` | five mutations of each of 37 operations |
| `a1_focused_inputs_convert` | the seven compile benchmarks: verified input, no refusal, v1 byte equality, VM parity with the specialized program |
| `a1_canonical_first_print` | first-print fixpoint, second cycle, allocation perturbation |
| `a1_locations_per_op` | per-operation records through print, two parses, export |
| `a1_location_mutations_name_their_op` | byte end, source label, origin, absence, derivation key |
| `a1_v1_exec_roundtrip` | production `exec` on exported v1, refusal of core text |
| `a1_normalized_outcomes_round_trip` | executable core holds no bridge operation and exports v1 |
| `a1_lifecycle_edges` | seven lifecycle mutations, each naming its event |
| `a1_inventory_is_closed` | registry equals registered operations and form decisions |
| `a1_conversion_is_total` | foreign operation, nested bridge operation, foreign type, dangling symbol, disabled rule, unsupported form |
| `a1_same_module_executes` | original VM, specialized VM, parsed-core VM, native O0 and release, traced lifecycle log |
| `a1_dead_scalar_preserves_events` | three operations removed, contract, locations, output, and trace unchanged |
| `a1_param_construction`, `a1_param_substitution`, `a1_param_identity`, `a1_param_cross_context_clone`, `a1_param_canonical_text` | the parameter attribute layer |
| `a1_benchmark_accounting` | budgets applied strictly at 1.19/1.20/1.21 and 1.29/1.30/1.31 |

The plan names one locations check. Its mutation half is the separate
`a1_location_mutations_name_their_op`.

An operation the gate's closure lacks takes its positive case, its
mutations, and its disabled rule from `benchmarks/compile/stdlib_heavy.mojo`,
whose closure holds all eleven.

Generated evidence under `target/pliron-a1/` (not tracked):

| File | sha256 prefix |
|---|---|
| `focused-inputs.tsv` (10 inputs) | `71570cf26e7b9dc2` |
| `corpus-inputs.tsv` (848 inputs) | `9786d69970e8c67e` |
| `op_inventory.tsv` | `ec59ff472241b113` |
| `malformed_matrix.tsv` | `dec81bdc5a702f99` |
| `gate.executable.txt` | `b476e999f623c79c` |
| `default.after.txt` | `43e6af8471c96062` |
| `workspace.after.txt` | `796388403cae6256` |

The same parsed module exported v1 text with sha256
`bec647ac…fe0b9a66c` (`same_module.tsv`).

### Resuming the measurement lane

Run alone, on AC power, on a quiet machine. Record governor, load, and
temperature with the results.

```sh
scripts/gen-pliron-a1-inputs
cargo build --release -j 2 --features pliron-a1 --example pliron_a1
rustc -Vv
uname -a
/opt/llvm-23/bin/llvm-config --version
sha256sum target/release/examples/pliron_a1
scripts/bench-pliron-a1 --bin target/release/examples/pliron_a1 --inputs target/pliron-a1/focused-inputs.tsv --out target/pliron-a1/focused --warmup 1 --runs 10 --order alternating
scripts/bench-pliron-a1 --bin target/release/examples/pliron_a1 --inputs target/pliron-a1/corpus-inputs.tsv --out target/pliron-a1/corpus --warmup 1 --runs 10 --order alternating
```

The second `bench-pliron-a1` command is the bulk lane. Both exit nonzero
while any input is refused, and write every raw sample before summarizing.
A refused input's diagnostic ends with every refusal the importer has for
it, counted. `scripts/cover-pliron-a1 --bin BIN --inputs FILE --out DIR
--jobs 4` is the coverage-only sweep, and the example's `--emit-core`,
`--emit-bridge` (which also verifies each function on its own, naming it
first), and `--emit-specialized` write the text a diagnostic names.

## Code footprint

Lines under `crates/mojito-pliron/src/a1/`, inline documentation included.

| Part | Files | Lines |
|---|---|---|
| Definitions | `ops.rs`, `types.rs`, `attrs.rs`, `inventory.rs` | 3,771 |
| Verification | `verify.rs`, `lifecycle.rs` | 2,078 |
| Conversion | `import.rs`, `export.rs`, `outcomes.rs`, `execute.rs` | 6,626 |
| Parameter attributes | `params.rs` | 829 |
| Text and locations | `text.rs`, `provenance.rs`, `ir_framework.rs` | 193 |
| Optimization | `opt.rs` | 143 |
| Measurement | `measure.rs` | 452 |
| Module root | `mod.rs` | 91 |
| **Total** | | **14,183** |

Outside the crate: `tests/pliron_a1_test.rs` 1,976,
`examples/pliron_a1.rs` 370, `scripts/bench-pliron-a1` 114,
`scripts/gen-pliron-a1-inputs` 93, `scripts/cover-pliron-a1` 131. The
VM's builtin name tables (`crates/mojito-vm/src/builtins.rs`, 118) are the
VM's own and stay whatever the verdict.

A1 deletes nothing. These lines are added beside the MIR they shadow.

## Removal

If the pivot is rejected, remove exactly:

- `crates/mojito-pliron/src/a1/` and its gated `pub mod a1` in `lib.rs`;
- crate feature `a1-core` and root feature `pliron-a1`;
- `examples/pliron_a1.rs` and its manifest entry;
- `tests/pliron_a1_test.rs` and its manifest entry;
- `scripts/bench-pliron-a1` and `scripts/gen-pliron-a1-inputs`.

Keep the gate fixture, `a1_reference_field_arithmetic_matches_vm`, and the
native operand fix. They stand without A1.

## Remaining boundaries

- **Coverage.** 887 of 893 corpus inputs convert; the six residues are
  named in §Coverage and filed on the roadmap by their kind.
- **Overhead.** No release measurement exists. The debug diagnostic cannot
  stand in for one.
- **Interpreter speed.** Both backends ran exported MIR. Nothing here says
  how fast a VM walking core operations would be.
- **Pin rehearsal.** Not run. The 0.17 to `477e6b0` upgrade in
  `pliron-stage6.md` and `pliron-promotion.md` is historical evidence only.
- **Broad gates.** The full suites, the corpus, the conformance sweeps, and
  the heavy Pliron lane were not run in this task.
