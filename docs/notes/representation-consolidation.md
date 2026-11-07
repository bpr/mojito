# Shrinking Generic Code Early: Fix Representations, Not Shapes

The playbook for the `pmir` work, in answer to: *how do we move Mojito
closer to Mojo in as few steps as possible, with the goal of downsizing the
program representation as early as possible to prevent code explosion
during generic instantiation?* Within that, the steps that come first are
the ones whose one fix closes or shrinks a large block of other entries.
The next step is always at the top. Finished steps are deleted;
`git log -- docs/notes/representation-consolidation.md` has them.

Entries are cited by stable ID and the order is computed, so every command
below stays valid as entries are filed and landed. Before a run, preview it
with `--dry-run` or `scripts/roadmap.py deps R1445`. Every command runs on
Opus. `--plan-fable-on-opus` gives a plan pass to every entry whose
`Model:` bullet says Fable, `Planned` or not. `--task` runs an entry after
its open prerequisites. `--only` runs exactly the entries named. A single
command cannot use both.

## Recommended order

1. **Finish gap D′** (size). R1440–R1444 landed 11-03, and the verifier
   rule (R1873, once R1445) landed 11-05 for a named `drop.reg` and a
   `deinit`-receiver call. Its unnamed `drop.reg` half is R1932, behind
   R359: the compiler's tuple storage, upstream's trivially destructible
   `!kgen.struct`, still destroys its elements through one.

2. **Gap F: members reached by name, not called** (size, the largest
   lever left). R1280 (the iteration step folds to the iterator's member),
   R1466 (a call on a struct value is its `__call__`), R1287 (`print` is
   library code), and R1461 landed 11-03. `mir::TYPE_REACHED_MEMBERS`
   still lists seven members, and each has entries:
   - `__call__`: R1933 (a call through a `def(...)` bound is a
     `call.indirect`; its witness tables landed with R1877 on 11-05,
     and its probe waits on R1934's `emit-mir`) and R1868 (`Variant`'s
     `init_with` handlers).
   - Display: `write_to` and `write_repr_to` left with R1882 (11-05;
     `String(x)` and `repr(x)` are library code). `write_string`, which
     the intrinsic receivers' `$write_formatted` reaches, goes with R1938.
   - The hash leaves (`__hash__`, `_update_with_simd`): R1937 (once `DType` is
     the library struct). Planned.
   - `__int__`: R1544 (`SIMD[dt, _](v)` binds no width from a vector).
   - R579 (a trait the pin does not declare) closes with it; R712
     landed 11-05.

   The lifecycle traits (`Copyable`, `Movable`, `Deinitable`, …) are not
   filed. The checker's copy and move synthesis is tied to them, and
   upstream declares them in `std/traits/`. R1832 (the synthesized `copy`
   calls `__mojito_fieldwise_copy`) is the first piece.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1933 --task R1868 \
     --task R1936 --task R1937 --task R1544 --task R579 -n 0
   ```

3. **Gap G's residues** (size, and conformance). R1436 landed 11-03: the
   check declares a closure's storage struct, and MIR lowers that
   declaration. R1434 deleted owned captures, and R1823 rejects the
   `capturing` effect and `@parameter` closures as the pin does. What is
   left:
   - R1892 (MIR names a closure and declares the binders the check
     collects for it as the struct's own, where upstream's front end
     does), then R1893 (the closure's checked
     type names its site, not its storage struct), then R1894 (captured
     origins as the struct's origin parameters).
   - R1889 deletes the compile-time capturing-callable environment, whose
     producers are gone.
   - Ownership: R1870 (a named closure at its last use is copied, not
     moved) and R1888 (a call through the storage conflicts with its own
     `mut` capture's loan).
   - R1349 (the inliner keeps a capturing nested `def`'s direct call).

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1894 --task R1889 --task R1870 \
     --task R1888 --task R1349 -n 0
   ```

4. **Gap H's residues** (partial size). R1474, R1475, R1895, and R1896
   landed 11-03: a condition compiles once with its binders in scope, and
   a `comptime if` carries its `Bool` value beside its constraint. R893
   and R1482 landed too. What is left:
   - One condition builder: R1901–R1904 move the constraint compiler's
     remaining operands onto the one builder, then R1905 drops the
     constraint beside MIR's `Bool` value. R1948 (a callable's `where`
     clauses compile twice, early for applications and again at its
     signature, after R1898) and R1899 (a bare pack receiver bound by
     spelling) sit beside them.
   - The elaborator: R1875, R1876 (a struct's origin parameters and its
     associated-type rows), and R1872 landed 11-05.
   - The check: R1842, R1871, R1856, and the generator follow-ups
     of R1486: R1907, R1908, R1909.
   - The members under *H* below were filed against `Elab`'s arms or the
     checker's folds. Re-probe each before planning it.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1905 --task R1948 --task R1899 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1842 --only R1871 --only R1856 \
     --only R1907 --only R1908 --only R1909 -n 0
   ```

5. **Gap I: packs are binder kinds** (slight size). Its cores landed
   11-03 (R1491, R1492, R1485, R1486). What is left:
   - R1910, then R1912: a value pack's, then a type pack's, reads come
     from the visible binder instead of being spelled again. Planned.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1912 -n 0
   ```

6. **The scalar, literal, and string structs' residues** (conformance;
   the old steps 1–3). `SIMD` and `Bool` are the declared structs below
   the checker (R1812–R1816, R1834, R1612), numeric literals' members and
   conversions are their declarations (R1753, R1785's first part, R1788,
   R1789, R1738), and a `StringLiteral[value]` is the declared struct
   (R1655, R1657–R1660). What is left:
   - `SIMD` storage: R1843 landed (splat and `SIMDLength` are the
     declared `@implicit` initializers' calls), R1955 (a `Float64`'s or `Bool`'s read), R1837 landed (a
     `literal.materialize` target is the storage only), and R1836 landed (a multi-lane literal
     converts at the whole vector storage).
   - `Bool`: R1840 landed (MIR's `i1` is a register type of its own),
     R1957 (the parameter domain's `i1` casts, after R1955), and R1841
     (`Bool` is an AST keyword).
   - Numeric literals: R1845 landed (a contextual conditional converts
     each arm), then R1848 → R1849 → R1850 → R1851 deletes the
     value-less literal type, its run-time register, and its compiler
     kind. R1850 landed: a literal register is zero-sized, and its
     readers take the number from its type. R1851 landed: the literal
     types are the declared structs' applications, and R1968 → R1969
     move their construction and member rules onto those structs. R1855 (`IntLiteral.__pow__`'s constraint) is beside them.
   - `StringLiteral`: R1858 landed (the elaborator fills no unsolved
     binder), and R1859 landed (a `[s: StringLiteral]` parameter is the
     auto-parameterized application). R1860 and R1861 landed: the
     value-less type is deleted, and R1862 deleted its descriptor;
     R1971 → R1972 → R1973 remove the remaining struct exclusions and the
     construction rule.
     R1863, R1864, and R1865 are independent.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1843 --task R1837 --task R1836 \
     --task R1957 --task R1841 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1968 --task R1969 --task R1855 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1862 --task R1863 --task R1864 --task R1865 -n 0
   ```

7. **Residues of the 10-27 to 10-31 `SIMD` runs** outside their work
   orders. Each is independent unless noted.
   - Compile-time applications: R1530 (displays and aggregates as
     parameter values), R1523 and R1527 (builtin bodies), R1574 (folding
     `SIMD` comparisons), R1563, R1587, R1588, and R1692 (an element
     list in a parameter position).
   - Parameter binding and conversion: R1537, R1541, R1545, R1540, R1565,
     R1571, R1572, R1604, R1548, and R1687 (`SIMDLength` into an `Int`
     binding). R1544 is under step 2.
   - `SIMD` storage and intrinsics: R1520, R1521, R1600, R1575, and
     R1734 (non-power-of-two widths).
   - Ownership and the VM: R1570, R1573, R1603, R1601, R1524. R1601 and
     R1603 are wrong-code bugs.
   - Syntax and library: R1598 (`assert`), R1602, R1564, R1607, R1605
     (after R792 and R178).
   - Function values (10-31): R1794 (a parametric struct's static) and
     R1796 (an initializer), with R1795 beside them.
   - The 11-01 gate fix: R1805 and R1806 (a module binding named like a
     callee's parameter wins over it), R1807 (three fixtures fail with the
     library linked from source), and R1808 (one source occurrence checked
     twice). R1809 (`reduce_add` stays out of line) is under *Inlining
     reach*.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1601 --task R1603 --task R1570 --task R1573 \
     --task R1524 --task R1598 --task R1602 --task R1564 --task R1607 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1530 --task R1523 --task R1527 --task R1574 \
     --task R1563 --task R1587 --task R1588 --task R1692 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1537 --task R1541 --task R1545 \
     --task R1540 --task R1565 --task R1571 --task R1572 --task R1604 --task R1548 --task R1687 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1520 --task R1521 --task R1600 --task R1575 \
     --task R1734 --task R1794 --task R1796 --task R1795 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1805 --only R1806 --only R1807 --only R1808 -n 0
   ```

## Where things stand (2026-11-03)

- Gaps A, B, and C have fully landed. Gap B's residues are conformance
  and cleanup: R1414 (a display's elements bind as call arguments), R695
  after it, R1415 (fieldwise keywords), and R1416 (hand-written MIR).
- Gap D′ has its verifier rule's unnamed `drop.reg` half left (R1932),
  behind R359.
- Of the 10-25 gaps, E has fully landed, and F, G, H, and I have their
  cores in. Each of those has residues, steps 2–5.
- Of the 10-25 conformance blocks, five have landed: type versus value
  (R1369–R1372), one subscript lvalue (R1373–R1375), conformance
  evidence (R1364, R1365), transfers (R1366, R1367), and abandonment
  (R1368). Their leftovers are under *Blocks without a size lever*. The
  scalar block is step 6.
- `SIMD`, `Bool`, `IntLiteral[value]`, `FloatLiteral[value]`, and
  `StringLiteral[value]` are the declared structs. The value-less literal
  types and their run-time registers remain (step 6).
- `print` is the bundled library function, and `Writable`'s members are
  the trait's own default bodies (R1287). `String(x)` resolves to the
  bundled initializers and `repr` is the library function (R1882).
- The differential pin is `1.3.0.dev2026110105` (11-01 re-pin). Its
  legacy-closure removal is implemented (R1823). Its other unimplemented
  changes, R1817–R1830, sit outside the gaps.

`enum` (10-25) moves no gap: each enum is an ordinary struct whose
members `mojito_parser::enums` generates as Mojo text, over the
`TyKind::Variant` intrinsic `__EnumStorage[*Ts]`. Its residues R1407,
R1408, R1409, and R1412 are under *Blocks without a size lever*, and
R1411 and R1413 under *Entries outside the gaps*.

| gap | upstream's shape | open core entries | entries that close or shrink | size lever |
|---|---|---|---|---|
| D′. Memory-only values live in registers | `__call_result_tmp__` storage, register-passable SSA only | R1932, behind R359 | about 15 (below) | yes |
| E. Compile-time values are slots | `kgen.param.constant` of the decl ref, no storage | none (R1874 landed 11-05) | R788, R378, R472, R508, R602 | yes |
| F. Members reached by name from the value's type | every operator and protocol step is a call of the requirement, folded at substitution; traits are library declarations | R1933, R1868, R1882, R1462, R1936, R1937, R1544 | about 20 (below) | yes, the largest |
| G. A capture-list closure is a capture list | a capture-list closure is a `__storage` struct of its captures, and its body a method of it | R1892 → R1893 → R1894 | R1349, R1870, R1888 | yes |
| H. Three compile-time evaluators | sema checks each expression once into a typed parameter expression; the elaborator alone evaluates it | R1901–R1904 → R1905, R1948 | about 25 (below) | partial |
| I. Packs are binder kinds | a pack is a `!kgen.param_list` value whose library members CTFE applies | R1910 → R1912 | about 20 (below) | slight |

### D′. Memory-only values live in registers

- **Upstream** (`UncheckedCallEmission.cpp`): a memory-only call result is
  written once into its destination or a synthesized
  `__call_result_tmp__`, and each consumer borrows or takes that storage.
  Only register-passable values are SSA registers (`CheckLifetimes`).
- **Mojito**: R1355, R1356, and R1440–R1444 moved the consumers onto the
  call-result slot, and R1418 (11-04) put every loan anchor and borrow
  slot's loans on it, closing R1331, R123, and R166. The verifier rejects
  the class (R1873), but for an unnamed `drop.reg` (R1932).
- **Still open from R1355's list**: R715 (a `^` argument), R1212
  (`$mat_r`), and R406 (a forwarded call-result subscript). Re-probe each.
- **Shrinks** R691, R1234, R118, R54, and R718.
- R473 stays open: its discard still copies, and a move into storage that
  dies unread runs.
- A raising call still hands its callee's error back in a register
  (R1438).

### F. Members reached by name, not called as the requirement

- **Mojito**: the backends dispatch a writer's `write_string`, the hash leaves, a scalar
  construction's `__int__`, and a `__call__` through a `def(...)` bound
  from the value's type, through `mir::TYPE_REACHED_MEMBERS`. Each such
  member keeps every argument, so it keeps one instance per `n`.
- **Upstream**: the traits are library declarations with default bodies,
  and every operator and protocol step is a `kgen.call` of the
  requirement, which substitution folds to the member.
- **Size**: R987 counts 868 `List.__len__` and 523 `Span.__len__`
  instances reached through trait-bound calls.
- **Closes**: R1933, R1868, R579 (R712 landed 11-05).
- **Shrinks** R987, R1291, R113, R1119, R25, R817, and R860.

### G. A capture-list closure is a capture list

- **Mojito**: every capture-list closure builds the pin's `__storage`
  struct, which the check declares, its body a method of it, and its
  value is the struct. Captures are by reference, and run-time
  `capturing[...]` values and `@parameter` closures are rejected as at the
  pin. The check collects the struct's binders, as upstream's
  `usesFromAbove` does; MIR still names the struct and declares them as
  its own (R1892), and the checked type names the closure's site (R1893).
- **Upstream**: a `F: def(...)` binder is instantiated at the struct
  type, so every call is direct. `kgen.create_closure` is made only for
  capture-free values.
- **Size**: an indirect call becomes direct, inlinable, and promotable.
- **Members**: R1349 is independent. The generic-closure entries (R71,
  R83, R452) are about naming parametric functions, not closure
  representation.

### H. Three compile-time evaluators

- **Mojito**: `mojito-comptime`'s evaluator is deleted (R1388), and a
  condition compiles once with its binders in scope (R1474, R1895,
  R1896). The constraint compiler (`compile_dependent_ct_expr`,
  `checker/constraints.rs`) still reads operands the one builder does not
  (R1901–R1904), and the lifted fallback (`Checker::lifted_application`)
  still folds.
- **Upstream** checks each expression once into a typed parameter
  expression. One elaborator evaluates it by CTFE, over any result type.
- **Members**, filed before R1388, to re-probe:
  - Former `Elab` arms: R1171, R1178, R1208.
  - `Self` and associated reads: R513, R436, R415.
  - Checker arms: R1199, R332, R1220, R448.
  - Domain: R480, R396, R1197.
  - Constructions: R1175.
  - Spelling: R388 and R398.
  - Library-side folds: R231, R197, and R198.
- Gap H also unblocks most of the reflection block below.

### I. Packs are binder kinds

- **Mojito**: `*Ts` and `*values` declare upstream's two binders, the
  hidden `.values` list and the visible `TypeList` / `ParameterList`
  (R1491, R1492). A bracket list with open slots is a partial
  `#kgen.bind_params` (R1485), and a type-bodied alias is a type generator
  (R1486). A pack's reads are still spelled again rather than read from
  the visible binder (R1910, R1912), and a compile-time `len`, subscript,
  and the `TypeList` vocabulary are answered by spelling
  (`pack_length_binder`, `typelist_receiver`, R1284).
- **Upstream**: a pack is a `TypeList` / `ParameterList` value, and its
  library members run by CTFE.
- **Members**:
  - Queries: R1284, R800, R904, R1182, R1921, R445, R803, R345, R804,
    R348, R1071.
  - Library ports: R810, R811, R812, R1181, R223.
  - Pack constraints: R1343.
  - Pack-typed parameters and display: R1490, R1489.

### Blocks without a size lever

Each is one cause, by roadmap reading, listed by number of members. They
move Mojito toward Mojo's shape and delete compiler special cases. Some
may raise instance counts: a library `SIMD` is real generic code.

| block | core | members |
|---|---|---|
| Scalars, `SIMD`, `Bool`, `DType`, literals, `StringLiteral`, and enum storage are compiler types | step 6 (R1843, R1955, R1837, R1957, R1841, R1845 → R1968 → R1969, R1858–R1860 → R1862), R1936 | R1408, R1019, R423, R1352, R851, R171, R427, R1681 (`Pointer`) |
| `Writable`'s intrinsic receivers (`DType`, `Error`) are host-formatted by each backend | R1936, R1938 | R1463, R1409, R18, R159, R1883, R1884 |
| Reflection is a builtin, not `Reflected[T]` | R324 | R271, R497, R272, R76, R444, R273, R900, R904, R1481 |
| A trait requirement has no checked signature | R758, R594 | R1407 (after R942, R549, R571), R595, R769, R640, R760, R549, R942, R1165, R67, R571, R284, R550, R570 |
| The checked handoff is rebuilt from span-keyed facts | R764 → R765 → R766 → R767 | R756, R757, R755, R626, R627, R754, R768 |
| Origins are dropped from `ParamDecl` | R761 → R762 → R763 | R1300, R1299, R1213, R674, R133, R353, R104, R242, R726 |
| Loans come from call shape, not type-carried origins | R87 | R88, R93, R122, R126, R304, R355, R1323, R94, R89, R1878 |
| Overload identity is a partial mangled string | R934 → R935 | R1172, R105, R64, R1294, R570, R1111 |
| Relocations name no `__moveinit__` | R1047, R341 | R731, R738, R732, R739, R740, R32, R1048 |
| One-shot parameter inference | R52 | R53, R537, R726, R61 |
| `Tuple` is `__RuntimeTuple` storage | R341, R359 | R343, R1121, R208, R1122, R207 |

The landed 10-25 blocks left one or two entries each, none with a core
to wait on:
- Type versus value: R383, R490.
- Conformance evidence: R792 (`conforms_to` takes one name and one
  trait), R1412.
- One lvalue: R1224. Transfers: R750. Abandonment: R302.

## Entries outside the gaps

None of these unblocks another entry. Run them whenever they are useful.

- **Inlining reach**: R1346, R1347, R1350, R1339, R1340, R1341, R1452
  (the pin measures a callee before cleaning what it spliced in), and
  R1809 (`SIMD.reduce_add` stays an out-of-line call).
- **Bugs**: R1112, R1035, R1289.
- **Hygiene**: R944, R868, R1276.
- **Library ports**: R1411 (`std.iter.map`), R1839 (`Set`'s `List`
  initializer), R1866 (upstream's string iterators), and R1906 (the owned
  `List` iterator). **Diagnostics**: R1413 (enum declaration errors).
- **Calls (11-01 to 11-03)**: R1853, R1854, R1857, R1838, R1833.
- **Divergences (11-01 to 11-03)**: R1900, R1869, R1890, R1852, R1885,
  R1913.
- **Performance**: R896, R897. **Pin legality**: R924.
  **Canonicalizer completeness**: R1255, R1256.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1346 --only R1347 --only R1350 \
  --only R1339 --only R1340 --only R1341 --only R1452 --only R1809 --only R1112 --only R1035 --only R1289 -n 0
scripts/claude_loop.py --model opus --plan-fable-on-opus --only R944 --only R868 --only R1276 \
  --only R896 --only R897 --only R924 --only R1255 --only R1256 -n 0
scripts/claude_loop.py --model opus --only R1411 --only R1839 --only R1866 --only R1906 --only R1413 -n 0
scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1853 --only R1854 --only R1857 --only R1838 \
  --only R1833 --only R1900 --only R1869 --only R1890 --only R1852 --only R1885 --only R1913 -n 0
```

## Measure

Corpus totals from `benchmarks/instantiation.tsv`, refreshed by
`scripts/instantiation-report`. The overnight gate says when it is stale.

| counter | 10-13 | 10-17 | 10-21 | 10-24 | 10-29 | 11-01 | 11-01 re-pin |
|---|---|---|---|---|---|---|---|
| fixtures | | | | 1,200 | 1,379 | 1,538 | 1,537 |
| concrete functions | 42,409 | 24,864 | 25,859 | 26,934 | 18,743 | 41,010 | 42,159 |
| concrete instructions | 892,391 | 402,905 | 386,579 | 401,799 | 859,746 | 1,821,334 | 2,108,848 |
| generators | 16,553 | 8,801 | 9,213 | 9,500 | 5,841 | 17,667 | 18,665 |
| generator instances | 26,868 | 13,804 | 13,989 | 14,619 | 9,164 | 26,483 | 27,496 |
| instances per generator | 1.62 | 1.57 | 1.52 | 1.54 | 1.57 | 1.50 | 1.47 |

- Compare runs with `scripts/instantiation-report --check`, over the
  fixtures both runs elaborate, not with the raw totals.
- The program is less than half its 10-13 size. Almost all of that came
  from dead code removal, inlining, and the unused-argument removal that
  landed 10-17.
- Instances per generator has barely moved: a generic body is still copied
  about as often as before. Gaps E, F, and G aim at that ratio.
- The library `SIMD` (10-27 to 11-01) is real generic code and inlined
  operator chains, and it doubled the program. Over the 1,368 fixtures
  both the 10-29 and 11-01 baselines elaborate, concrete functions grew
  2.0× and instructions 1.85×, and generators and their instances about
  2.7×. That is the size gaps D′–I now have to win back.
- The 11-01 re-pin baseline follows the step-1 chain (R1798–R1801,
  R1811) and the re-pin's library ports. Over the 1,525 fixtures both
  11-01 baselines elaborate, concrete instructions grew 16% and generators
  6%. The largest single jump is the ported `_powf_scalar`'s
  large-exponent branch (`simd_pow_dunders.mojo`, 35,102 to 84,689
  instructions); the rest is spread across the corpus.
- No baseline has been taken since the 11-01 re-pin, so the 11-01 to
  11-03 runs (the old steps 1–9) are unmeasured. Refresh it with
  `scripts/instantiation-report` before choosing between steps 1–6.
- `assets/ok/pointer_tuple_field_write.mojo` elaborates differently from
  run to run (R1069), so `--check` can report a few instructions of
  growth there with no change.

## Landed so far

- The first four representation gaps (10-16/17): direct nested `def`
  calls, defaults filled at call binding, synthesized lifecycle calls,
  and one `inline-parametric` substitution.
- The region chain (10-20/21): every phase from the ownership checks to
  Pliron on `hlcf` regions, with `Mem2Reg`, `SROA`, `cse`, and
  unused-argument removal following.
- The second pass (10-21 to 10-24): gaps A, B, and C, and gap D except
  R1440–R1445.
- 10-25 to 10-26: gap H's chain (R1379–R1388), gap F's three stages,
  gap G's cores, and the stale-entry sweeps (R1400–R1406).
- 10-26 to 10-27: the conformance blocks for type versus value, the
  subscript lvalue, conformance evidence, transfers, and abandonment
  (R1364–R1375).
- 10-27 to 10-28: `Int`, `UInt`, and `Float64` as width-one `SIMD`
  (R1389–R1391), `pop`/`index` ops and LLVM intrinsics (R1392–R1394,
  R1516, R1517, R1519, R1546, R1547), builtin folds (R1395, R1522,
  R1525, R1526), the declared `SIMD`
  and its member ports (R1397, R1549–R1555, R1578–R1581, R1589), and
  `__mlir_type.i1` (R1188).
- 10-28 to 10-31: the rest of the `SIMD` member ports (R1503, R1561,
  R1562, R1566, R1567, R1583–R1586, R1662, R1671, R1673, R1675), its
  display in Mojo (R276), R1398 and R1399, the declared struct in the
  checker (R1557, R1591, R1791, R1792), declared `IntLiteral[value]` and
  `FloatLiteral[value]` (R1779–R1781), and R1558.
- 11-01: the 11-01 gate fix, and the library `SIMD`'s last members:
  `%`, `divmod`, and `ceildiv` (R1623, closing R850), native display
  through `write_to` (R1664), an initializer called without inlining
  (R1617), `SIMD(copy=x)` (R1793), and `_dynamic_shuffle` (R1777).
  Then `SIMD` as the declared struct below the checker: `!kgen.simd`
  storage in the verifier and both backends (R1798, R1799), `TyKind::Simd`
  deleted (R1800, R1810), the `_mlir_value` field read (R1811), and
  builtin ops over the storage (R1801).
- 11-01 to 11-03: `SIMD` built from its storage below the elaborator
  (R1812–R1816, R1834); `Bool` as the declared struct (R1608–R1612,
  R1189); literal members and conversions as declarations (R1753, R1788,
  R1789, R1738, part of R1785); `StringLiteral[value]` as the declared
  struct (R1651, R1654, R1655, R1657–R1660); gap D′'s consumers
  (R1440–R1444); gap E's callable binder read and, 11-05, its pack of
  callables (R1874); gap F's iteration step,
  `__call__`, and `print` (R1280, R1466, R1287, R1461); gap G's storage
  struct in the check and the pin's closure removal (R1430, R1433, R1434,
  R1436, R176, R1823); gap H's conditions (R1474, R1475, R1895, R1896,
  R893, R1482); and gap I's cores (R1485, R1486, R1491, R1492).
