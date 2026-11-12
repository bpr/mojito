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
with `--dry-run` or `scripts/roadmap.py deps <ID>`. Every command runs on
Opus. `--plan-fable-on-opus` gives a plan pass to every entry whose
`Model:` bullet says Fable, `Planned` or not. `--task` runs an entry after
its open prerequisites. `--only` runs exactly the entries named. A single
command cannot use both.

## Recommended order

The structural cores of every gap have landed. Gap G is finished, and gaps
H and I are down to residues. What is left with a size lever is gap F's
last two members and gap D′'s verifier half. Everything after step 2 is
conformance residue. The program also grew 1.76× between the 11-01 re-pin
and 11-04 (*Measure*), so the size steps still matter.

1. **Gap F: members reached by name, not called** (size, the largest
   lever left). `__call__` on a struct value, `__int__`, `write_to`,
   `write_repr_to`, and the iteration step are requirement calls now (R1466,
   R1544, R1882, R1280), and so are `write_string` (R1938) and the hash
   leaves, which only a dead `Variant` storage shape kept (R2010): the
   type-reached member list is gone. Every call of a struct value is its
   `__call__` method call and the elaborator devirtualizes none (R2102).
   Left:
   - The lifecycle traits (`Copyable`, `Movable`, `Deinitable`, …) are not
     filed. Their first piece landed with R1832: the synthesized `copy` is
     upstream's `Self(copy=self)` for every struct.

2. **Gap D′ is finished**: R1932 (11-08) made the verifier reject an
   unnamed `drop.reg` of a memory-only register as well.

3. **Residues of the 10-27 to 10-31 `SIMD` runs** not yet run. The
   ownership, syntax, and compile-time application entries landed 11-06 to
   11-07. Each entry here is independent unless noted.
   - Parameter binding and conversion: R1540, R1565, R1571,
     R1572, and R1548 (R1687, `SIMDLength` into an `Int`
     binding, and R1541, operator-pattern unification, landed 11-07).
   - `SIMD` storage and intrinsics: R1520, R1521, R1600, and R1575
     (R1734, non-power-of-two widths, landed 11-08).
   - Function values (10-31): landed 11-08 (R1796, an initializer, R1794,
     a parametric struct's static, and R1795, an instance method).
   - The 11-01 gate fix: R1807 (three fixtures fail with the library
     linked from source) and R1808 (one source occurrence checked twice)
     (R1805, a module binding named like a parameter in `origin_of`, and
     R1806, the same collision in a conditional conformance, landed 11-08).
   - R1605 (`any`/`all` over `Iterable`) waits on R792 and R178.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus \
     --task R1540 --task R1565 --task R1571 --task R1572 --task R1548 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1520 --task R1521 --task R1600 --task R1575 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1807 --only R1808 -n 0
   ```

4. **Follow-ups filed by the 11-06 to 11-07 runs.** Each names the entry
   whose landing found it.
   - Compile-time applications: R1989 and R1988 (an element or field read
     off a compile-time temporary is copied, where the pin loads it; R1988
     after R1501), R1990 (a subscript in a signature), R1991 and R1992
     (builtin bodies; R1992 after R1960), R1993 → R1994 and R1995
     (compile-time `SIMD` calls).
   - Ownership: R1976 (a register-passable receiver of a subscript or
     `in` keeps its place), R1977 (a field read out of a temporary), R1986
     → R1985 (`List`'s Mojito-only `__setitem__`, also after R1216), and
     R1987 (a loop or `try` that never falls through).
   - Native: R1984 (a `RegisterPassable` struct held as its one field),
     R1974.
   - `assert` and the library: R1978, R1979 (Planned) → R1980, R1981,
     R1982, R1983. R1975 (two `Some[Trait]` parameters).

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1989 --only R1988 --only R1990 \
     --only R1991 --only R1992 --only R1993 --only R1994 --only R1995 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1976 --only R1977 --only R1986 \
     --only R1985 --only R1987 --only R1984 --only R1974 --only R1975 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1978 --only R1979 --only R1980 \
     --only R1981 --only R1982 --only R1983 -n 0
   ```

5. **The scalar, literal, and string structs' residues** (conformance).
   The follow-ups filed while their earlier entries landed:
   - `Bool`: the parameter domain reads its storage, and its operators
     are its builtin dunders over it (R1955, landed 11-09), and its `i1`
     casts fold (R1957, landed 11-09).
   - Numeric literals: their construction, copy, and conformances are the
     declared struct's (R1968, R1969, landed 11-08), and the struct rules
     see its application (R2064, landed 11-09).
   - `StringLiteral`: the struct rules see its application (R1972,
     landed 11-08), and its construction is the declared `__init__`
     (R1973, landed 11-09).

6. **Gap H's and I's residues** (partial and slight size).
   - Gap H: R1948 (a callable's `where` clauses compile twice) and R1949
     (a `contains` element naming a binder is re-read by spelling). Then
     re-probe the members under *H* below before planning any.
   - Gap I: its members under *I* below. R1284 (compile-time `len`)
     landed 11-09: a pack's `len` is the library application; its
     residues are R2076 and R2077.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1948 --only R1949 --only R1284 -n 0
   ```

## Where things stand (2026-11-07)

- Gaps A, B, C, E, and G have fully landed. Gap B's residues are
  conformance and cleanup: R1414 (a display's elements bind as call
  arguments), R695 after it, R1415 (fieldwise keywords), and R1416
  (hand-written MIR).
- Gap D′'s verifier rule is whole (R1932, 11-08).
- Gap F has only the unfiled lifecycle traits left (step 1). Gaps H and I
  have
  their cores and their binder reads in. Their members are filed against
  shapes that have since changed, so each needs a re-probe.
- Of the 10-25 conformance blocks, five have landed: type versus value
  (R1369–R1372), one subscript lvalue (R1373–R1375), conformance
  evidence (R1364, R1365), transfers (R1366, R1367), and abandonment
  (R1368). Their leftovers are under *Blocks without a size lever*.
- `SIMD`, `Bool`, `IntLiteral[value]`, `FloatLiteral[value]`, and
  `StringLiteral[value]` are the declared structs, and the value-less
  literal types are deleted (R1851, R1860). Step 5 moves their last
  construction rules onto the structs.
- `print` is the bundled library function, `Writable`'s members are the
  trait's own default bodies (R1287), and `String(x)` and `repr` are
  library code (R1882). `assert` parses and lowers to the bundled
  `debug_assert` (R1598).
- The differential pin is `1.3.0.dev2026110105` (11-01 re-pin). Its
  legacy-closure removal is implemented (R1823). Its other unimplemented
  changes, R1817–R1830, sit outside the gaps.
- The overnight gate is paused (11-06): it took about nine hours, mostly
  in corpus groups that re-check std per fixture. R1996–R1999 (`tooling`)
  cut that and add a ten-minute tier between `scripts/smoke` and the gate.

`enum` (10-25) moves no gap: each enum is an ordinary struct whose
members `mojito_parser::enums` generates as Mojo text, over the
`TyKind::Variant` intrinsic `__EnumStorage[*Ts]`. Its residues R1407,
R1408, R1409, and R1412 are under *Blocks without a size lever*, and
R1411 and R1413 under *Entries outside the gaps*.

| gap | upstream's shape | open core entries | entries that close or shrink | size lever |
|---|---|---|---|---|
| D′. Memory-only values live in registers | `__call_result_tmp__` storage, register-passable SSA only | none | about 15 (below) | yes |
| E. Compile-time values are slots | `kgen.param.constant` of the decl ref, no storage | none | R788, R378, R472, R508, R602 | yes |
| F. Members reached by name from the value's type | every operator and protocol step is a call of the requirement, folded at substitution; traits are library declarations | none filed (lifecycle traits) | about 20 (below) | yes, the largest |
| G. A capture-list closure is a capture list | a capture-list closure is a `__storage` struct of its captures, and its body a method of it | none | none | landed |
| H. Three compile-time evaluators | sema checks each expression once into a typed parameter expression; the elaborator alone evaluates it | R1948, R1949 | about 25 (below) | partial |
| I. Packs are binder kinds | a pack is a `!kgen.param_list` value whose library members CTFE applies | R2077 | about 20 (below) | slight |

### D′. Memory-only values live in registers

- **Upstream** (`UncheckedCallEmission.cpp`): a memory-only call result is
  written once into its destination or a synthesized
  `__call_result_tmp__`, and each consumer borrows or takes that storage.
  Only register-passable values are SSA registers (`CheckLifetimes`).
- **Mojito**: R1355, R1356, and R1440–R1444 moved the consumers onto the
  call-result slot, and R1418 (11-04) put every loan anchor and borrow
  slot's loans on it, closing R1331, R123, and R166. The verifier rejects
  the class (R1873, R1932).
- **Still open from R1355's list**: R715 (a `^` argument), R1212
  (`$mat_r`), and R406 (a forwarded call-result subscript). Re-probe each.
- **Shrinks** R691, R1234, R118, R54, and R718.
- R473 stays open: its discard still copies, and a move into storage that
  dies unread runs.
- A raising call still hands its callee's error back in a register
  (R1438).

### F. Members reached by name, not called as the requirement

- **Mojito**: no backend reaches a member by name from the value's type
  any more but the lifecycle members (R1938 and R2010 landed). A call
  through a `def(...)` bound, generic or not, is the bound's `__call__`
  requirement call (R1933, R2005). A call of any closure's storage is
  its `__call__` method call (R2003, R2101), and the verifier rejects an
  indirect call of a struct value (R2102).
- **Upstream**: the traits are library declarations with default bodies,
  and every operator and protocol step is a `kgen.call` of the
  requirement, which substitution folds to the member.
- **Size**: R987 counts 868 `List.__len__` and 523 `Span.__len__`
  instances reached through trait-bound calls.
- **Shrinks** R987, R1291, R113, R1119, R25, R817, and R860.

### G. A capture-list closure is a capture list

- **Landed** (11-06). Every capture-list closure builds the pin's
  `__storage` struct, which the check declares and names, its body a
  method of it, and its value is the struct. Captures are by reference,
  with their origins as the struct's origin parameters (R1894). Run-time
  `capturing[...]` values and `@parameter` closures are rejected as at the
  pin.
- **Upstream**: a `F: def(...)` binder is instantiated at the struct
  type, so every call is direct. `kgen.create_closure` is made only for
  capture-free values.
- **Size**: an indirect call becomes direct, inlinable, and promotable.
- **Left**: the generic-closure entries (R71, R83, R452) are about
  naming parametric functions, not closure representation.

### H. Three compile-time evaluators

- **Mojito**: `mojito-comptime`'s evaluator is deleted (R1388), and a
  condition compiles once with its binders in scope (R1474, R1895,
  R1896), and the constraint compiler's operands are the one builder's
  (R1901–R1905). A callable's `where` clauses still compile twice
  (R1948), the constraint compiler (`compile_dependent_ct_expr`,
  `checker/constraints.rs`) still re-reads a `contains` element by
  spelling (R1949), and the lifted fallback
  (`Checker::lifted_application`) still folds.
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
  (R1486). A pack's reads come from the visible binder (R1910, R1912).
  A compile-time `len` is the library application (R1284); a subscript
  and the `TypeList` vocabulary are still answered by spelling
  (`typelist_receiver`).
- **Upstream**: a pack is a `TypeList` / `ParameterList` value, and its
  library members run by CTFE.
- **Members**:
  - Queries: R800, R904, R1182, R445, R345, R2076,
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
| Scalars, `SIMD`, `Bool`, `DType`, literals, `StringLiteral`, and enum storage are compiler types | R1936 | R1408, R1019, R1352, R851, R171, R427, R1681 (`Pointer`) |
| `Writable`'s intrinsic receivers (`DType`, `Error`) are host-formatted by each backend | R1936 | R1463, R1409, R18, R159, R1883, R1884 |
| Reflection is a builtin, not `Reflected[T]` | R324 | R271, R497, R272, R76, R444, R273, R900, R904, R1481 |
| A trait requirement has no checked signature | R758, R594 | R1407 (after R942, R549, R571), R595, R769, R640, R760, R549, R942, R1165, R67, R571, R284, R550, R570 |
| The checked handoff is rebuilt from span-keyed facts | R764 → R765 → R766 → R767 | R756, R757, R755, R626, R627, R768 |
| Origins are dropped from `ParamDecl` | R761 → R762 → R763 | R1300, R1299, R1213, R674, R133, R353, R104, R242, R726 |
| Loans come from call shape, not type-carried origins | R87 | R88, R93, R122, R126, R304, R355, R1323, R94, R89, R1878 |
| Overload identity is a partial mangled string | R934 → R935 | R1172, R105, R64, R1294, R570, R1111 |
| Relocations name no `__moveinit__` | R1047, R2027, R2025 | R731, R738, R732, R739, R740, R32, R1048 |
| One-shot parameter inference | R52 | R53, R537, R726, R61 |
| `Tuple` is `__RuntimeTuple` storage | R2025, R2028 | R343, R1121, R208, R1122, R207 |

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
- **Bugs**: R1112, R1289.
- **Hygiene**: R944, R868, R1276.
- **Library ports**: R1411 (`std.iter.map`), R1839 (`Set`'s `List`
  initializer), R1866 (upstream's string iterators), and R1906 (the owned
  `List` iterator). **Diagnostics**: R1413 (enum declaration errors).
- **Calls (11-01 to 11-03)**: R1853, R1854, R1857, R1838, R1833.
- **Divergences (11-01 to 11-03)**: R1900, R1869, R1890, R1852, R1885,
  R1913.
- **Performance**: R896. **Pin legality**: R2081.
  **Canonicalizer completeness**: R1255, R1256.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1346 --only R1347 --only R1350 \
  --only R1339 --only R1340 --only R1341 --only R1452 --only R1809 --only R1112 --only R1289 -n 0
scripts/claude_loop.py --model opus --plan-fable-on-opus --only R944 --only R868 --only R1276 \
  --only R896 --only R2081 --only R1255 --only R1256 -n 0
scripts/claude_loop.py --model opus --only R1411 --only R1839 --only R1866 --only R1906 --only R1413 -n 0
scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1853 --only R1854 --only R1857 --only R1838 \
  --only R1833 --only R1900 --only R1869 --only R1890 --only R1852 --only R1885 --only R1913 -n 0
```

## Measure

Corpus totals from `benchmarks/instantiation.tsv`, refreshed by
`scripts/instantiation-report`. The overnight gate says when it is stale.

| counter | 10-13 | 10-17 | 10-21 | 10-24 | 10-29 | 11-01 | 11-01 re-pin | 11-04 | 11-08 |
|---|---|---|---|---|---|---|---|---|---|
| fixtures | | | | 1,200 | 1,379 | 1,538 | 1,537 | 1,578 | 1,662 |
| concrete functions | 42,409 | 24,864 | 25,859 | 26,934 | 18,743 | 41,010 | 42,159 | 62,479 | 63,777 |
| concrete instructions | 892,391 | 402,905 | 386,579 | 401,799 | 859,746 | 1,821,334 | 2,108,848 | 3,804,799 | 4,265,051 |
| generators | 16,553 | 8,801 | 9,213 | 9,500 | 5,841 | 17,667 | 18,665 | 31,552 | 32,485 |
| generator instances | 26,868 | 13,804 | 13,989 | 14,619 | 9,164 | 26,483 | 27,496 | 44,529 | 47,114 |
| instances per generator | 1.62 | 1.57 | 1.52 | 1.54 | 1.57 | 1.50 | 1.47 | 1.41 | 1.45 |

- Compare runs with `scripts/instantiation-report --check`, over the
  fixtures both runs elaborate, not with the raw totals.
- The program is less than half its 10-13 size. Almost all of that came
  from dead code removal, inlining, and the unused-argument removal that
  landed 10-17.
- Instances per generator has barely moved: a generic body is still copied
  about as often as before. Gaps E, F, and G aim at that ratio, and E
  and G have landed without moving it much.
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
- The 11-04 baseline was taken with the 11-04 gate repair, after the
  11-01 to 11-03 runs: library `print`, `Bool`, and `StringLiteral`, and
  the cores of gaps D′ to I. Over the 1,537 fixtures both it and the
  11-01 re-pin elaborate, concrete functions grew 1.45×, instructions
  1.76×, generators 1.65×, and their instances 1.58×. The growth is
  spread: most fixtures gained a few thousand instructions, the shape a
  library `print` reached from every program would leave. Which change
  accounts for how much is not measured.
- The 11-08 baseline (`ed68e3aab`, after pmir batch 6) follows the 11-05
  to 11-08 runs. Over the 1,578 fixtures both it and the 11-04 baseline
  elaborate, concrete functions fell to 0.97×, generators to 0.98×, and
  their instances held at 1.01×, while concrete instructions grew 1.07×.
  Instances per generator rose from 1.41 to 1.45 over the whole corpus,
  which gained 84 fixtures.
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
- 11-05 to 11-06: gap D′'s verifier rule for a named `drop.reg` (R1873);
  gap F's `__int__`, `Variant`'s handlers, display, and two traits the
  pin does not declare (R1544, R1868, R1882, R1462, R579, R712); gap G's
  residues (R1349, R1870, R1888, R1889, R1892–R1894); gap H's one
  condition builder and its check entries (R1898, R1899, R1901–R1905,
  R1842, R1856, R1871, R1872, R1875, R1876, R1907–R1909); gap I's binder
  reads (R1910–R1912); and the scalar and literal residues: `SIMD`
  storage (R1836, R1837, R1843), MIR's `i1` and the `Bool` keyword
  (R1840, R1841), the value-less literal types deleted (R1845,
  R1848–R1851, R1855), and `StringLiteral`'s (R1858–R1865).
- 11-06 to 11-07: the `SIMD` runs' ownership, syntax, and compile-time
  application residues (R1601, R1603, R1570, R1573, R1524, R1598, R1602,
  R1564, R1607, R1530, R1523, R1527, R1574, R1563, R1587, R1588, R1692,
  R1729).
