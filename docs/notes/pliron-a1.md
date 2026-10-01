# Pliron A1 — Shadow `mojito.core` Proof (record)

Record of the Stage A1 vertical slice from
[`docs/pliron-backend-pivot-plan.md`](../pliron-backend-pivot-plan.md)
§Smallest falsifiable proof.

## Status and decision

**`PASS-SLICE`, and `NO` for the pivot decision: F5 fails.** Recorded
2026-09-27; the decision corpus run 2026-09-29; overhead measured
2026-09-30.

- All seven proof items pass on the gate fixture, on the VM and natively.
- F1 to F4 pass. **F5 (overhead) fails** on the predeclared stress row:
  Gate ×64 compiles 1.62 times as slowly under the shadow (1.54 to 1.69 at
  three median absolute deviations), against a limit of 1.20
  (§Measurements). Peak memory is unchanged, 1.00 on every row.
- By the plan's decision procedure that is a NO for disproportionate
  measured overhead. What was measured is this bridge: a shadow that
  prints and re-parses its canonical text on every compile. That round
  trip is 79 to 85% of the added time on the failing rows.
- A2 is not started. The experiment was removed from the tree on
  2026-09-30 after the owner accepted the NO (§Removal).
- All ten focused inputs convert, and all 893 inputs of the decision
  corpus (§Coverage).
- The pin rehearsal passed on 2026-09-30 (§Pin rehearsal): upstream
  `81155d9` needed ten changed lines, all inside the adapter.
- The focused measurement lane ran. The bulk lane did not, because it
  cannot reverse a stress row that fails alone (§Measurements).

The MIR waist, the v1 `.mir` schema, the VM, and the native backend's MIR
input are unchanged. A1 was optional and default off; it is removed.

## Verdict table

| Criterion | Result | Evidence |
|---|---|---|
| F1: represent and verify the slice cleanly | PASS | `a1_representation_contract`, `a1_malformed_ops_are_diagnostics` (185 cases, 0 accepted, 0 panics), `a1_same_module_executes` |
| F2: locations, lifecycle, artifact fidelity | PASS | `a1_locations_per_op`, `a1_location_mutations_name_their_op`, `a1_lifecycle_edges`, `a1_canonical_first_print`, `a1_v1_exec_roundtrip`, `a1_dead_scalar_preserves_events` |
| F3: conversion totality | PASS | `a1_inventory_is_closed`, `a1_conversion_is_total`, `a1_focused_inputs_convert` |
| F4: default-lane isolation | PASS | both default graphs byte-identical to the captured ones, no `pliron`, `pliron-llvm`, or `llvm-sys` package, default build succeeds with LLVM discovery removed |
| F5: disproportionate overhead | **FAIL** | Gate ×64 time ratio 1.62, lower bound 1.54, limit 1.20; `stdlib_heavy` 1.45; Gate ×16 straddles the limit; memory passes everywhere (§Measurements) |
| Full canonical-artifact compatibility | 893 / 893 | `scripts/cover-pliron-a1` over `corpus-inputs.tsv`, the full census re-run 2026-09-30 at `1967fe1d` (§Coverage) |
| Pin-maintenance rehearsal | PASS | `477e6b0` to `81155d9` in under an hour, 10 lines in 4 adapter files and 2 test files, no escape, no fork (§Pin rehearsal) |

Core text is 1.56 to 2.00 times the v1 text of the same module, under the
2.0 line that makes a design review mandatory; it was 2.6 to 3.4 before the
review (finding 6). Size is not a substitute for the time and memory
thresholds.

## Pins and environment

| | |
|---|---|
| Repository | slice: `d846604a59d8d93353cee9f2d22a6cfed767265f` plus that task's changes; measurements: `1967fe1dbd0f85e940cfb32063cc4afc81e43251` plus the harness change of finding 19 |
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
- The wire namespace is `mojito`. Pliron identifiers admit no dot, and an
  operation id has one separator. Attribute keys carry no dialect prefix
  (`provenance`, `reg`, `slot`).
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
  way out.
- A `return` or `escape` inside a `finally` body overrides the pending
  outcome, as the VM's `exec_try` has it. It is an ordinary exit: that try
  has no cleanup or `finally` left to run, the pending outcome goes unused,
  and the exit runs the drops of the outer tries itself. A `finally` body
  that never falls off has no `resume`, so the continuations of the exits
  pending on it stay in the graph unreached, and export still checks them.
- A return carrying cleanup across a finally is refused by name. Overriding
  it would owe its roots, which needs a dispatch on the pending outcome at
  the override site, as the native lowering's `emit_pending_resolution`
  does. No corpus input reaches it.

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
  stable identity, so a parse restores it. The module and each function
  are named by the full key (`main||0|0|Function`), every other operation
  by its key local to its function (`|0|3|Primary`).
- Canonical text leaves out the `identity` attribute, which the location
  already names: the canonical print strips it, and `text::parse_text`
  restores it from the locations before verifying.
- A source record names its source by index into the module tables'
  `sources`, and prints only the parts it has:
  `at(3:1521:1529) origin(4) "source"`. A contract event and a
  `derived_from` name an operation of the same function by local key.
- In the gate: 143 operations with a source record, 37 derived, 110 absent.

### Canonical text

`C(module)` is the print of the module after a parse into a fresh context.
Pliron names values by arena index, so text depends on allocation history.
The parser allocates in the order of the text's structure, which no name and
no history affects. `C(m) == C(parse(C(m)))` holds from the first parse.

The wrapper's first line is `mojito-a1-core 1`. Production `exec` refuses
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
   canonicalization and the fresh parse are 780 ms of 870 ms, and 651 ms
   after the text-size review. In a release build they are 26 and 17 ms.
6. **Core text was large; the design review cut it to at most 2.0 times
   v1.** It was 2.6 to 3.4 times v1. On the gate, the `identity` attribute
   repeated what the location table already said (15% and 14% of the
   bytes), the dialect prefix was 17.5%, attribute keys 13%, and every
   provenance repeated an absolute source path. The cuts are in
   §Locations: identity only in the location, function-local keys,
   indexed sources, provenance parts printed only when present, dialect
   `mojito`, unprefixed keys. `empty` sits at 2.00 (19,431 / 9,716),
   because its module tables and `__toplevel__` slots are a fixed cost.
   Text size was not what the boundary's time follows: bytes fell 41% on
   the gate, canonicalize plus parse about 17% (debug, uncontrolled).
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
    `verify_dominance` judges every use itself, skipping dead blocks. Still
    true at upstream `81155d9` (§Pin rehearsal). A local Pliron fix is
    ready but not submitted.
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
    never verified by the native backend. All three are fixed in
    `crates/mojito-native/src/native/mono/`: specialization now runs
    `mir::verify` on its output and refuses a program that fails it.
17. **Three more specialized programs did not verify.** An indirect call
    kept the callable-contract target of the template's binders
    (`__call__$ov$T$AnyType`), which the specializer now respells from the
    instance's contract. The other two were the verifier's: it read a
    `ref` element slot's store as a write through the reference, and
    compared a capturing closure to the unqualified callable storage its
    instance erased to by the strict value-coercion rule rather than by
    the environment predicate it had already applied.
16. **`#[format]` prints a variant's name before its own literal.** A
    tuple variant with a custom format holding a literal word printed the
    word twice (`Taketake`); the default tuple format is the one to use.
18. **A named type argument dropped a `Tuple`'s elements.** A
    constructor's type argument reaches MIR as a string naming the struct
    (`Set[Tuple[Int, String]]()`), so it binds `Tuple$t2[…]` without the
    element arguments the receiver's type carries and the instance's owner
    spells. The instance's declaration and its body then named two
    `List` instances. The spelled argument now leaves a binder the
    receiver solved alone. Specialization also stopped being
    VM-transparent where the VM names a template the specialized program
    no longer declares: the slice bound's `Optional` and a string
    literal's `String.__hash__`. The VM now takes both from the instance.

19. **The harness credited the shadow a smaller output.** Each mode
    printed and hashed its own result: the baseline the whole compiled
    program (about 8 MB of v1 text, the bundled library included), the
    shadow only its exported closure. That took about 150 ms off every
    shadow sample, so small inputs measured 0.91 to 0.93. Every mode now
    prints and hashes the compiled program before the shadow starts. The
    first batch is kept as `focused-asymmetric-consumption/`; it fails the
    same two rows.

## Coverage

One shadow run per focused input, coverage only. Byte counts re-taken
2026-09-30 after the text-size review, release build.

| Input | Converted | Functions | Operations | v1 bytes | Core bytes | Ratio |
|---|---|---|---|---|---|---|
| Gate | yes | 13 | 360 | 69,504 | 117,895 | 1.70 |
| `add` | yes | 6 | 213 | 39,172 | 65,493 | 1.67 |
| `empty` | yes | 2 | 67 | 9,716 | 19,431 | 2.00 |
| `hello` | yes | 5 | 201 | 36,411 | 61,801 | 1.70 |
| `generic` | yes | 29 | 845 | 186,488 | 290,844 | 1.56 |
| `tuple` | yes | 50 | 1,509 | 341,984 | 548,268 | 1.60 |
| `tstring` | yes | 43 | 1,456 | 328,015 | 530,372 | 1.62 |
| `stdlib_heavy` | yes | 172 | 6,028 | 2,092,497 | 3,448,285 | 1.65 |
| Gate ×16 | yes | 133 | 2,775 | 578,367 | 987,141 | 1.71 |
| Gate ×64 | yes | 517 | 10,503 | 2,212,971 | 3,799,826 | 1.72 |

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
| Exits crossing a try, with and without `finally` | 887 | 6 |
| A module constant no longer reaches a bundled `comptime for` index (2026-09-30) | 888 | 5 |
| Specialized MIR verifies (2026-09-30; the three inputs re-run alone, debug build) | 891 | 2 |
| A named type argument keeps a receiver's `Tuple` spelling (2026-09-30; the input re-run alone, debug build) | 892 | 1 |
| An exit from a `finally` body is an ordinary exit (2026-09-30; the input re-run alone, debug build) | 893 | 0 |
| The full census re-run at `1967fe1d` (2026-09-30, release build) | **893** | **0** |

The last input, `assets/ok/pliron_finally_overrides.mojo`, is pinned by
`a1_finally_override_converts`. The re-run's rows are
`target/pliron-a1/corpus-coverage-1967fe1d/coverage.tsv` (sha256
`7516c320e0bf28c5`).

Still refused by name, because no corpus input reaches them: a subscript
store through a slice, a return carrying cleanup across a finally, a
call with unresolved parameters, and the four rejected forms.

Beyond conversion, the exported programs of
`assets/ok/keyword_slice_subscripts.mojo`, `with_statement.mojo`,
`path_operations.mojo`, and `tuple_hashable_dict_key.mojo` run on the VM
as the original programs do. The VM builds a slice bound as the
`Optional` instance its projection declares, and hashes a string literal
through the `String.__hash__` instance minted for its hasher, where it
looked both up by the template's name, which specialization drops.

## Measurements

### Diagnostic only

One process per mode, **debug build, uncontrolled machine**. Not a verdict.

| Gate, `--phase compile` | Wall | Peak |
|---|---|---|
| baseline | 8.23 s | 295,916 KiB |
| shadow | 8.41 s | 295,636 KiB |
| shadow-opt | 8.44 s | 295,948 KiB |

Exclusive phase times of the shadow-opt run, in milliseconds:

| Text | front end | specialize | construct | verify bridge | normalize | verify core | DCE | verify after DCE | canonicalize | parse | export | verify MIR |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| before the size review | 7,521 | 4.3 | 9.4 | 14.5 | 11.7 | 15.9 | 0.9 | 15.8 | 409.7 | 371.0 | 23.7 | 0.3 |
| after (2026-09-30) | 7,704 | 4.9 | 9.3 | 14.4 | 13.1 | 17.7 | 0.9 | 16.8 | 356.2 | 295.3 | 23.7 | 0.3 |

Before the size review, the dead-scalar test removed 3 of 302 operations
and 1,340 of 169,661 bytes in 3 sweeps.

### Focused lane, 2026-09-30

Release build, ten alternating pairs after one warmup, one fresh process per
sample, wall time and peak memory from `wait4`
(`scripts/bench-pliron-a1`, `--phase compile`). The ratio is the median of
the ten paired shadow-to-baseline ratios, and its bounds are three median
absolute deviations either side. Limits: time strictly below 1.20, memory
strictly below 1.30.

| Input | Baseline ms, median / p90 | Shadow ms, median / p90 | Time ratio (lower to upper) | Peak KiB, baseline / shadow | Memory ratio upper | Verdict |
|---|---|---|---|---|---|---|
| Gate | 1,907 / 1,916 | 1,930 / 1,955 | 1.016 (0.994 to 1.039) | 288,256 / 288,476 | 1.004 | pass |
| `empty` | 1,902 / 1,925 | 1,863 / 1,876 | 0.979 (0.952 to 1.006) | 287,112 / 287,676 | 1.002 | pass |
| `hello` | 1,875 / 1,915 | 1,880 / 1,904 | 0.989 (0.962 to 1.015) | 287,020 / 288,036 | 1.006 | pass |
| `add` | 1,909 / 1,934 | 1,883 / 1,912 | 0.988 (0.947 to 1.029) | 287,272 / 287,332 | 1.002 | pass |
| `generic` | 2,030 / 2,053 | 2,131 / 2,149 | 1.046 (1.035 to 1.058) | 305,968 / 306,132 | 1.004 | pass |
| `tuple` | 1,936 / 1,955 | 2,141 / 2,178 | 1.104 (1.084 to 1.124) | 292,436 / 292,280 | 1.001 | pass |
| `tstring` | 1,979 / 1,986 | 2,174 / 2,195 | 1.103 (1.081 to 1.125) | 298,836 / 298,104 | 1.001 | pass |
| `stdlib_heavy` | 2,749 / 2,778 | 3,995 / 4,026 | **1.455 (1.423 to 1.486)** | 401,468 / 401,268 | 1.002 | **time fails** |
| Gate ×16 | 2,069 / 2,104 | 2,439 / 2,491 | 1.187 (1.143 to 1.231) | 306,652 / 306,376 | 1.002 | time inconclusive |
| Gate ×64 | 2,605 / 2,649 | 4,192 / 4,222 | **1.617 (1.544 to 1.689)** | 363,900 / 363,808 | 1.002 | **time fails** |
| All ten, summed per run | 20,954 / 21,008 | 24,633 / 24,796 | 1.177 (1.165 to 1.189) | 401,468 / 401,268 | 1.002 | pass |
| Full decision corpus | not run | | | | | see below |

With the dead-scalar pass (`shadow-opt`) the same rows read 1.008, 0.983,
0.990, 0.992, 1.040, 1.111, 1.098, **1.478**, 1.183 (inconclusive),
**1.632**, and 1.180 summed. The verdicts are the same.

Median exclusive phase times of the shadow, in milliseconds
(`metrics/*.jsonl`; verify is the bridge, core, and exported-MIR
verifications together):

| Input | Operations | Front end | Specialize | Construct | Normalize | Verify | Canonicalize + parse | Export | DCE (opt) | Added |
|---|---|---|---|---|---|---|---|---|---|---|
| Gate | 360 | 1,681 | 1.0 | 1.0 | 1.2 | 3.4 | 39.0 | 2.7 | 0.1 | 48 |
| `empty` | 67 | 1,659 | 0.5 | 0.4 | 0.2 | 0.7 | 6.8 | 0.5 | 0.0 | 9 |
| `hello` | 201 | 1,658 | 0.7 | 0.7 | 0.7 | 1.9 | 20.9 | 1.5 | 0.1 | 26 |
| `add` | 213 | 1,663 | 0.7 | 0.7 | 0.7 | 2.0 | 22.0 | 1.6 | 0.1 | 28 |
| `generic` | 845 | 1,801 | 3.0 | 1.9 | 2.6 | 7.5 | 93.7 | 6.2 | 0.5 | 115 |
| `tuple` | 1,509 | 1,715 | 4.0 | 3.1 | 5.0 | 14.7 | 178.0 | 11.7 | 0.9 | 217 |
| `tstring` | 1,456 | 1,746 | 3.8 | 2.9 | 4.9 | 14.4 | 173.7 | 11.6 | 0.8 | 211 |
| `stdlib_heavy` | 6,028 | 2,395 | 48.6 | 12.3 | 19.7 | 59.7 | 1,054.9 | 52.1 | 3.4 | 1,247 |
| Gate ×16 | 2,775 | 1,821 | 7.1 | 5.2 | 8.6 | 25.3 | 317.4 | 20.4 | 0.8 | 384 |
| Gate ×64 | 10,503 | 2,297 | 76.1 | 19.7 | 33.4 | 104.9 | 1,230.0 | 87.7 | 3.4 | 1,552 |

What the numbers say:

- **The text round trip dominates.** Canonicalize plus parse is 79% of the
  added time on Gate ×64 and 85% on `stdlib_heavy`: 0.12 and 0.18 ms per
  operation, of 0.15 and 0.21 added in all. Construction is 1%, all
  verification 5 to 7%, export 4 to 6%, specialization 4 to 5%.
- **The baseline is mostly fixed cost.** About 1.66 s of every compile is
  the front end over the bundled library, whatever the program. The shadow
  scales with the entry's specialized closure. A small program hides the
  shadow under that floor, and a program whose closure reaches ten
  thousand operations does not.
- **Memory is not the problem.** The peak is the front end's, and the
  shadow never exceeds it, though it holds both representations.
- The remaining shadow-below-baseline medians (`empty`, `hello`, `add`) are
  inside their own noise bounds. The wall clock includes process teardown,
  which the phase timers do not.
- Without the text round trip the Gate ×64 shadow would add about 320 ms
  to 2,300, about 1.12. That is arithmetic on the phase medians, not a
  measurement, and the plan does not let it stand in for one.

**Boundary alone** (`--phase artifact`: both modes load the frozen v1 text of
the whole compiled program, `target/pliron-a1/artifacts/`, so the front end
is a 0.23 to 0.37 s load). Reported, never the budget:

| Input | Baseline ms | Shadow ms | Time ratio (lower to upper) |
|---|---|---|---|
| Gate | 502 | 516 | 1.021 (0.981 to 1.062) |
| `empty` | 493 | 466 | 0.950 (0.933 to 0.966) |
| `hello` | 522 | 506 | 0.959 (0.841 to 1.078) |
| `add` | 499 | 493 | 0.985 (0.973 to 0.998) |
| `generic` | 543 | 623 | 1.149 (1.093 to 1.205) |
| `tuple` | 516 | 718 | 1.405 (1.365 to 1.446) |
| `tstring` | 532 | 723 | 1.348 (1.307 to 1.389) |
| `stdlib_heavy` | 792 | 2,094 | 2.649 (2.527 to 2.770) |
| Gate ×16 | 557 | 929 | 1.676 (1.624 to 1.729) |
| Gate ×64 | 677 | 2,274 | 3.388 (3.223 to 3.553) |
| All ten, summed per run | 5,705 | 9,392 | 1.654 (1.610 to 1.699) |

**Why the bulk lane did not run.** The budget binds the corpus aggregate
and each predeclared stress row. Gate ×64's lower bound is above the limit,
so F5 fails whatever the aggregate is, and ten pairs over 893 inputs is
about 18 hours of serial compiles that could not change the verdict. The
corpus aggregate is therefore **unmeasured**, not passed. The census's
single uncontrolled runs (4 jobs, 2026-09-29, before the text-size review)
put it near 1.14, with 177 of 887 inputs at or above 1.20; that is a
diagnostic only. Gate ×16 straddles the limit and was not repeated, for the
same reason.

**Machine record** (`target/pliron-a1/machine.txt`). Intel Core i7-10875H,
Linux 7.1.1, AC power, governor `powersave` (unchanged), Rust 1.96.1,
LLVM 23.1.0. Load average 1.1 to 1.3 during both lanes, package
temperature 52 to 59 °C, no throttling. The machine was not idle: 14 GB in
use, and 6.6 GB already in zram swap before the run, with no swap growth
during it. The noise bounds are what that cost.

| File | sha256 prefix |
|---|---|
| `target/release/examples/pliron_a1` | `f80df789516bf5b9` |
| `focused/samples.tsv` (300 samples, all ok) | `a6979636a632c6e8` |
| `focused-artifact/samples.tsv` | `ddc31bf5bb9194cc` |
| `focused-asymmetric-consumption/samples.tsv` (finding 19, superseded) | `0a6f1f02d0be8038` |
| `artifacts/inputs.tsv` | `fa16e7f0fa0c2ace` |
| `corpus-inputs.tsv` (893 inputs) | `f0d9184268decc7c` |

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
| `a1_finally_override_converts` | a `return` in a `finally` body: no refusal, v1 byte equality, VM parity; a malformed unreached continuation is a diagnostic |
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
| `corpus-inputs.tsv` (893 inputs) | `f0d9184268decc7c` |
| `op_inventory.tsv` | `ec59ff472241b113` |
| `malformed_matrix.tsv` | `dec81bdc5a702f99` |
| `gate.executable.txt` | `b476e999f623c79c` |
| `default.after.txt` | `43e6af8471c96062` |
| `workspace.after.txt` | `796388403cae6256` |

The same parsed module exported v1 text with sha256
`bec647ac…fe0b9a66c` (`same_module.tsv`).

### The measurement lane

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

The first `bench-pliron-a1` command is the focused lane of §Measurements.
The second is the bulk lane, which was not run. The artifact lane first
writes each focused input's compiled v1 text with `--mode baseline
--emit-v1` into `target/pliron-a1/artifacts/`, lists those files with their
hashes in `inputs.tsv`, and passes `--phase artifact`. Both exit nonzero
while any input is refused, and write every raw sample before summarizing.
A refused input's diagnostic ends with every refusal the importer has for
it, counted. `scripts/cover-pliron-a1 --bin BIN --inputs FILE --out DIR
--jobs 4` is the coverage-only sweep, and the example's `--emit-core`,
`--emit-bridge` (which also verifies each function on its own, naming it
first), and `--emit-specialized` write the text a diagnostic names.

## Pin rehearsal

Run 2026-09-30, roadmap 1.1, per the A1 plan's §5.11: a disposable
`git clone --local` of `857b929d` in a scratch directory with its own
target directory, discarded afterward. The main tree's pin did not move.

| | |
|---|---|
| From | `477e6b0edb18b29df4cf7b90f0f468dc8a872f22` (0.17.0) |
| To | `81155d96b8797d149de1d88219c431a68a75ef33` (upstream `master` by `git ls-remote`, 0.18.0 plus 21 commits; 37 commits, 76 files) |
| Toolchain | unchanged: `llvm-sys` 231, LLVM 23.1.0, Rust 1.96.1 |
| Wall clock | 4 minutes from re-pin to a clean build of every target, about 40 minutes to the last probe |
| Changed | 3 pins, 10 source lines, 1 lock entry |

**Breaks and adaptations**, each in the adapter or its gated tests:

1. Pliron itself failed to build. It uses `Hash` on `combine`'s
   `SourcePosition`, which `combine` gained only in 4.6.8, while its
   manifest still asks for `combine = "4"` and our lock held 4.6.7.
   Fixed with `cargo update -p combine --precise 4.6.8`, lock only.
2. `dict_key!` now declares a plain `static Identifier`, where it declared
   a `LazyLock` (`7fd181d`, `7758a2f`, compile-time `ident!`). Four
   dereferences in `a1/outcomes.rs` and three in `tests/pliron_a1_test.rs`
   dropped their `*`.
3. `erase_given_names` takes `&Context`, where it took `&mut Context`.
   Clippy flagged the three calls (`a1/measure.rs`, `a1/text.rs`, the
   production backend's `lib.rs`) and the crate tests' `canonical_text`
   helper; each lost a `mut`.

Nothing escaped `crates/mojito-pliron` and its feature-gated tests: no
other crate, no MIR, checker, or VM change, no `src/main.rs` change, and no
fork. Canonical text, parameter attributes, and snapshots did not move.

**Probes at the new pin**, all passing: the 20 named A1 checks of §Probes
and commands (the malformed matrix and inventory closure included),
`a1_reference_field_arithmetic_matches_vm`, the crate's 27 tests but one
that fails at `477e6b0` too (below), clean Clippy with `-D warnings` over
every target, and VM and native `run` agreeing byte for byte on the gate
fixture, `assets/ok/pliron_finally_overrides.mojo`, and
`benchmarks/compile/stdlib_heavy.mojo`. Both default dependency graphs
were byte-identical to the main tree's, with no `pliron` or LLVM package.

**Finding 13 upstream.** Swapping `verify_dominance` for Pliron's own
`verify_value_dominance` at `81155d9` panics on 12 of the 54 `try`-bearing
`assets/ok` fixtures, at `graph/dominance.rs:342` (`DomTree::dominates`
indexes a block the dominator map lacks). Upstream `b869e77` taught the
dominance *frontier* to skip unreachable blocks; the use/def check still
does not. A fix in the owner's local Pliron checkout (branch
`dominance-unreachable`, not submitted) follows LLVM and MLIR: a block
unreachable from the entry is dominated by every block and dominates only
itself. With it, Pliron's own check accepts all 12 fixtures. `verify_dominance`
can go once a pin carries that fix.

**Found on the way.** `capability::tests::pliron_instr_capabilities_cover_the_instruction_vocabulary`
failed at both pins: `857b929d` gave `SizeOf` and `ConstructTypeParam`
their MIR mnemonics (`layout.size_of`, `type.construct`) without capability
rows. Both rows are now in `capability.rs` and
`conformance/pliron-capability.tsv`.

Raw logs, the adaptation diff, and the graph captures are under
`target/pliron-a1/rehearsal-81155d9/` (not tracked).

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

The owner accepted the rejection and this was removed on 2026-09-30:

- `crates/mojito-pliron/src/a1/` and its gated `pub mod a1` in `lib.rs`;
- crate feature `a1-core` and root feature `pliron-a1`;
- `examples/pliron_a1.rs` and its manifest entry;
- `tests/pliron_a1_test.rs` and its manifest entry;
- `scripts/bench-pliron-a1`, `scripts/gen-pliron-a1-inputs`, and
  `scripts/cover-pliron-a1`;
- the A1 navigation entries in `docs/symbol-map.md`.

Keep the gate fixture, `a1_reference_field_arithmetic_matches_vm`, and the
native operand fix. They stand without A1.

## Remaining boundaries

- **Corpus aggregate.** Unmeasured. The bulk lane was skipped because a
  stress row fails alone; nothing here says the aggregate passes.
- **A cheaper canonical form.** The failing cost is the print and parse
  that first-print stability needs at this pin (finding 4). A shadow
  without it was not built or measured.
- **Machine state.** One batch per lane, on a machine in ordinary use. The
  failing rows clear the limit by more than twice their noise bound.
- **Interpreter speed.** Both backends ran exported MIR. Nothing here says
  how fast a VM walking core operations would be.
- **Pin rehearsal.** One of the pivot plan's two rehearsals is done
  (§Pin rehearsal). The main tree stays at `477e6b0`.
- **Removal.** Not done. The experiment stays in the tree until the owner
  has reviewed this record ([`docs/roadmap.md`](../roadmap.md) 1.1).
- **Broad gates.** The full suites, the corpus, the conformance sweeps, and
  the heavy Pliron lane were not run in this task.
