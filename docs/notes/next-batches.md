# Next Batches After `pmir`

The batches to run after the bracket/parameter-application unification
(cluster A) and the compile-time values cluster (cluster B), in order. Each
batch is one `scripts/claude_loop.py` command. `--task` runs each named entry
after its open prerequisites, so a batch may run a few entries it does not
name. Preview one with `--list` or `--dry-run`. `scripts/pmir-batches` reads
the commands below, one batch each, and runs focused and the instantiation
ratchet after every batch. Delete a batch here once it has landed.

## 1. Reading a struct's `comptime` members through a type (11 entries)

A struct's `comptime` value and type members, read through the type name, an
application, a bound, or a local compile-time value: in signatures, trait
methods, annotations, and compile-time expressions. Library code reads
members this way everywhere.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R436 --task R1178 --task R1195 --task R1208 --task R1199 --task R1217 \
  --task R1218 --task R1219 --task R1484 --task R1639 --task R2023
```

## 2. Generators (7 entries)

Upstream's generator semantics: a constrained constant is a
`__generator_type`, a default is evaluated once at compile time, generator-typed
parameters and trait requirements, a nested `def` applied as its generator,
an `out`-result function as a compile-time application, and instances keyed
so an unread default is never evaluated.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R509 --task R528 --task R1487 --task R1488 --task R1693 --task R1736 --task R1778
```

## 3. Conformance refinement in `comptime if` (4 entries)

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

## 4. The MIR call op as upstream's `lit.call` (6 entries)

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

## 5. Built-ins declared in the library (6 entries)

`Pointer`, `VariadicPack`, and `VariadicList` become library structs over
`!kgen` types, as upstream declares them, instead of compiler rules, along
with upstream's litref builtins. R221 runs as a prerequisite.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R1924 --task R1927 --task R1922 --task R1681 --task R1314
```

## 6. Declarations named from their resolved signatures (4 entries)

Trait members and overloads are named from their typed signatures, not
re-spelled from syntax, and a trait default is checked over its trait's
declaration rather than as a module-scope `def` the check builds.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus -n 0 \
  --task R594 --task R595 --task R1770 --task R1771
```
