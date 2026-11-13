# Next Batches After `pmir`

The batches to run after the bracket/parameter-application unification
(cluster A) and the compile-time values cluster (cluster B), in order. Each
batch is one `scripts/claude_loop.py` command. `--task` runs each named entry
after its open prerequisites, so a batch may run a few entries it does not
name. Preview one with `--list` or `--dry-run`. `scripts/run-batches` reads
the commands below, one batch each, and runs focused and the instantiation
ratchet after every batch. Delete a batch here once it has landed.

The first three batches repair what the 2026-11-13 overnight gate still
fails, so the gate can go green before the structural work resumes. Each
of their entries names the fixture or test it turns green; a fixture that
is red until then is commented out in `tests/focused/list.txt` behind its
entry's ID, and the session that lands the entry restores the line.

## Checking between batches by hand

`scripts/run-batches` runs these checks itself. To run a batch by hand and
check it before the next:

1. Run the batch's `scripts/claude_loop.py` command below. It commits each
   entry it lands.
2. Run the focused tier, about four minutes:

   ```sh
   export LLVM_SYS_231_PREFIX="$(scripts/llvm-prefix)"
   export PATH="$LLVM_SYS_231_PREFIX/bin:$PATH"
   scripts/focused              # VM and native; `--vm-only` needs no LLVM
   ```

   It exits 0 when every entry passes. On a failure it prints
   `FAIL fixture <path>` or `FAIL test <target>::<name>`, and the full report
   is in `target/focused/last.log`. `scripts/focused --filter NAME` reruns
   just the entries matching `NAME` after a fix.
3. Check that the batch's newly green items stay green. A batch below lists
   the commented-out `tests/focused/list.txt` lines it should restore. Make
   sure the landed sessions uncommented them (`git diff HEAD~N --
   tests/focused/list.txt`), then rerun `scripts/focused --filter` on them.
4. Check the instantiation ratchet:

   ```sh
   scripts/instantiation-report --check
   ```

   "stale" or a small intended "growth" means you should commit the refreshed
   baseline: `cp target/instantiation.tsv benchmarks/instantiation.tsv`.
   A "lost" fixture is a regression to triage before the next batch.
5. Optionally, for a broad check in under a minute: `scripts/smoke`.

The overnight gate (`scripts/run-overnight-gates`) remains the full check.
`scripts/focused --from-gate target/overnight-gates/<stamp>/check-default.log`
prints the list lines a gate run's new failures add.

## 1. Gate repair: folds Mojo always performs (4 entries)

With the pre-elaboration pipeline off, `@always_inline("builtin")` literal
and `Pointer` operators reach the VM as calls it cannot run, a call location
names the callee, and two indexer and `Bool`-lane conversions go missing.
Upstream performs these as part of elaboration, never as an optional pass.
This batch fixes most of the `pre_elaboration::*` corpus trials and three
`evaluator_test` tests.
Focused lines to restore:
- R2157: `pliron_int_truediv`, `literal_operator_dunders`, `owned_pointer_api`,
  and the test `power_and_true_division`.
- R2158: `reflection_source_location`.
- R2017: `indexer_normalization`, `conversion_traits_library`.
- R2018: `bool_declared_operators`.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R2157 --task R2158 --task R2017 --task R2018
```

## 2. Gate repair: the remaining pipeline-off failures (4 entries)

Single-fixture failures with the pipeline off:
- an instance symbol collision for a generator-typed argument;
- an unfolded `SIMDLength` quotient in `SIMD.deinterleave`;
- an unverified `_reflection_write_to` instance;
- a lifted `comptime for` that passes its pack by register.

Focused lines to restore: `generator_runtime_value` (R2159),
`simd_slice_insert_split_interleave` (R2160), `writable_trait_library`
(R2161).

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R2159 --task R2160 --task R2161 --task R2002
```

## 3. Gate repair: the other red tests and sweeps (5 entries)

- R2162: nested numeric constructors check in exponential time, so
  `float16_semantics` times out in the assets sweep.
- R2163: a list-display subscript skips the bounds check; the gate's native
  unit test and the pin's abort text both pin it.
- R2164: a module-scope `comptime` type member read through a static
  method's result does not resolve.
- R2165: an uninitialized `unsafe_alloc` take prints `0` instead of trapping.
- R2067: `dtype_dynamic_parameter_rejected` gets the pin's "cannot use a
  dynamic value in a parameter list".

Focused lines to restore: `float16_semantics` (R2162), and the tests
`a_type_member_is_read_through_a_module_alias_and_a_construction` (R2164)
and `unsafe_uninit_length_slots_are_never_written_until_stored` (R2165).

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R2162 --task R2163 --task R2164 --task R2165 --task R2067
```

## 4. Reading a struct's `comptime` members through a type (11 entries)

A struct's `comptime` value and type members, read through the type name, an
application, a bound, or a local compile-time value: in signatures, trait
methods, annotations, and compile-time expressions. Library code reads
members this way everywhere.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R436 --task R1178 --task R1195 --task R1208 --task R1199 --task R1217 \
  --task R1218 --task R1219 --task R1484 --task R1639 --task R2023
```

## 5. Generators (7 entries)

Upstream's generator semantics: a constrained constant is a
`__generator_type`, a default is evaluated once at compile time, generator-typed
parameters and trait requirements, a nested `def` applied as its generator,
an `out`-result function as a compile-time application, and instances keyed
so an unread default is never evaluated.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R509 --task R528 --task R1487 --task R1488 --task R1693 --task R1736 --task R1778
```

## 6. Conformance refinement in `comptime if` (4 entries)

A `comptime if conforms_to(...)` arm refines what the body may assume: an
arm assumes the earlier conditions failed, `conforms_to` works over a
`def`'s parameter and a thin function, and a type proposition is a run-time
value. Generic library code branches this way constantly. After this batch,
comptime is solid enough to move on; what remains (packs, reflection,
diagnostic wording, the compile-time memory model) is demand-driven.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R1244 --task R280 --task R1147 --task R1650
```

## 7. The MIR call op as upstream's `lit.call` (6 entries)

A call reads its argument conventions from the callee's signature rather
than checker markers, passes by-address operands as addresses with no
`arg_places` side table, and returns a raising callee's result and error
through memory slots. The largest structural change left at the MIR waist;
it touches the VM, Pliron, and the package format. R1918 and R1919 run as
prerequisites.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R1419 --task R1920 --task R1438 --task R1420
```

## 8. Built-ins declared in the library (6 entries)

`Pointer`, `VariadicPack`, and `VariadicList` become library structs over
`!kgen` types, as upstream declares them, instead of compiler rules, along
with upstream's litref builtins. R221 runs as a prerequisite.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R1924 --task R1927 --task R1922 --task R1681 --task R1314
```

## 9. Declarations named from their resolved signatures (4 entries)

Trait members and overloads are named from their typed signatures, not
re-spelled from syntax, and a trait default is checked over its trait's
declaration rather than as a module-scope `def` the check builds.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R594 --task R595 --task R1770 --task R1771
```
