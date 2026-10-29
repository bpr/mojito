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

1. **Finish the library `SIMD`** (conformance, in progress). The run
   started 10-27 landed R1389–R1397, the `SIMD` declaration, and its
   member ports (R1549–R1555, R1578–R1581, R1589, R1594, R1595). It was stopped
   on 10-28 after R1188 for the overnight gate. Its remaining work order,
   which reruns from R1503 (R1566 landed `//`; its `%`, `divmod`, and
   `ceildiv` half is R1623, after R184's `/`):
   - R1503, R1561, R1562, R330, R184, R1623, R1567, R1582: initializers,
     the lane subscript, target queries, lane gathers, `/`, the rest of
     division, `**`, and `__round__`.
   - R296, R1583, R1584, R1585, R1586: the `reduce` tree and what it needs.
   - R276 (`SIMD`'s text in Mojo), then R1398 (delete the dead `SIMD`
     member rules) and R1399 (`Int(x)`, `UInt(x)`, `Float64(x)` as
     `SIMD.__init__` overloads; it has no open prerequisite).

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1398 --task R1399 -n 0
   ```

2. **`SIMD` as the declared struct everywhere** (the port's end).
   - R1557 makes `SIMD` and its aliases the declared struct in the
     checker, after R1556 (no `TyKind::Simd` pattern matches outside
     `mojito-types`), R1591 (after R240), R1592, R1593, R1399,
     R1507, R276, and R330.
   - R1558 then holds a `SIMD` value as its `!kgen.simd` field in MIR and
     both backends.
   - R1596 (after R66) and R1597 (after R1592) finish the surface.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1558 --task R1596 --task R1597 -n 0
   ```

3. **`Bool` as a library struct** (R1188's residue). R1188 made
   `__mlir_type.i1` a type of its own and split the port:
   - R1608 declares `Bool` and its members.
   - R1609 (initializers), R1610 (operators), and R1611 (printing, after
     R432) follow it.
   - R1612 retires `TyKind::Bool`, after those and R1189.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1612 -n 0
   ```

4. **Finish gap D** (size). R1356 landed in part on 10-26. Its census
   filed the shapes that still destroy a memory-only register as R1440
   (reference results copied for a `read` parameter), R1441 (builtin
   arguments), R1442 (operands off the call-result path), R1443 (copied
   consuming receivers), and R1444 (callable binder copies), with probes
   on R1435 and R1095. R1445, the verifier rule, comes last.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1445 -n 0
   ```

5. **Gap E: compile-time values are slots** (size, small). R662 and R663
   landed. A callable binder's read is still a frame slot (R1449), and
   `remove-unused-params` still counts slot reads (R1451).

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1449 --task R1451 -n 0
   ```

6. **Gap F: members reached by name, not called** (size, the largest
   lever left). Its three stages have landed: the bundled library
   declares the compiler-known traits, operators are calls of the
   requirement, and the elaborator reads the witness table. What is left:
   - The members a backend still reaches from a value's type
     (`mir::TYPE_REACHED_MEMBERS`): display (R1287, R422); the `DType`,
     `Bool`, and `StringLiteral` hash leaves (R160, R1608, R1192); a
     scalar construction's `__int__` (R1399); and a `Variant` handler's
     `__call__` (R1444, R1466).
   - A struct callee's `def(...)` contract still composes its `__call__`
     (R1466).
   - R1280: the iteration step cannot inline.
   - R1461: `Writable`'s synthesized reflective defaults become the
     trait's own bodies.

   The lifecycle traits (`Copyable`, `Movable`, `Deinitable`, …) are not
   filed. The checker's copy and move synthesis is tied to them, and
   upstream declares them in `std/traits/`.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1280 --task R1466 --task R1287 --task R1461 -n 0
   ```

7. **Gap G: a capture-list closure is a capture list** (size). Its cores
   landed (R1376–R1378, R1427, R1428, R533, R1344, R1345). What is left:
   - R1436 moves the closure's storage struct into the check.
   - R1433 → R176 → R1434 withdraw run-time `capturing[...]` values,
     which still convert the struct to a closure of its body.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1436 --task R1434 -n 0
   ```

8. **Gap H's residues** (size, partial). The chain R1379–R1388 landed:
   `mojito-comptime` evaluates nothing, and the check binds every
   `comptime` declaration. What is left:
   - R1474 (a condition compiled without its binders in scope) and R1475
     (a `comptime if` condition kept as a `GenericConstraint`).
   - R1480 (a package's module constants exported as an `Elaborated`
     table) and R1482 (the bundled `List`'s field layout).
   - R893 stops an instance-budget blowup.
   - The members under *H* below were filed against `Elab`'s arms or the
     checker's folds. Re-probe each before planning it.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1474 --task R1475 --task R1480 --task R1482 --task R893 -n 0
   ```

9. **Gap I: packs are binder kinds** (slight size, about 20 entries):

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1491 --only R1492 --only R1485 --only R1486 -n 0
   ```

10. **Residues of the 10-27/28 `SIMD` run** outside its work order. Each
    is independent unless noted.
    - Compile-time applications: R1528 → R1529 → R1530 (displays and
      aggregates as parameter values), R1523 and R1527 (builtin bodies),
      R1574 (folding `SIMD` comparisons), R1563, R1587, R1588.
    - Parameter binding and conversion: R1537, R1538, R1541, R1544,
      R1545, R1540, R1606, R1565, R1571, R1572, R1604, R1548.
    - `SIMD` storage and intrinsics: R1520, R1521, R1536, R1600, R1575.
    - Ownership and the VM: R1570, R1573, R1603, R1601, R1524. R1601 and
      R1603 are wrong-code bugs.
    - Syntax and library: R1569 (unary `+`), R1598 (`assert`), R1602,
      R1564, R1607, R1605 (after R792 and R178).

    ```sh
    scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1601 --task R1603 --task R1570 --task R1573 \
      --task R1524 --task R1569 --task R1598 --task R1602 --task R1564 --task R1607 -n 0
    scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1530 --task R1523 --task R1527 --task R1574 \
      --task R1563 --task R1587 --task R1588 -n 0
    scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1537 --task R1538 --task R1541 --task R1544 \
      --task R1545 --task R1540 --task R1606 --task R1565 --task R1571 --task R1572 --task R1604 --task R1548 -n 0
    scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1520 --task R1521 --task R1536 --task R1600 \
      --task R1575 -n 0
    ```

## Where things stand (2026-10-28)

- Gaps A, B, and C have fully landed. Gap B's residues are conformance
  and cleanup: R1414 (a display's elements bind as call arguments), R695
  after it, R1415 (fieldwise keywords), and R1416 (hand-written MIR).
- Gap D has R1440–R1445 left.
- Of the 10-25 gaps, E, F, G, and H have their cores in; I has R1491,
  R1492, R1485, and R1486 left.
- Of the 10-25 conformance blocks, four have landed: type versus value
  (R1369–R1372), one subscript lvalue (R1373–R1375), conformance
  evidence (R1364, R1365), transfers (R1366, R1367), and abandonment
  (R1368). Their leftovers are under *Blocks without a size lever*. The
  scalar block is steps 1–3.
- The stale-entry sweeps R1400–R1406 have landed.

`enum` (10-25) moves no gap: each enum is an ordinary struct whose
members `mojito_parser::enums` generates as Mojo text, over the
`TyKind::Variant` intrinsic `__EnumStorage[*Ts]`. Its residues R1407,
R1408, R1409, and R1412 are under *Blocks without a size lever*, and
R1410, R1411, and R1413 under *Entries outside the gaps*.

| gap | upstream's shape | open core entries | entries that close or shrink | size lever |
|---|---|---|---|---|
| D′. Memory-only values live in registers | `__call_result_tmp__` storage, register-passable SSA only | R1440–R1444, then R1445 | about 15 (below) | yes |
| E. Compile-time values are slots | `kgen.param.constant` of the decl ref, no storage | R1449, R1451 | R788, R378, R472, R508, R602 | yes |
| F. Members reached by name from the value's type | every operator and protocol step is a call of the requirement, folded at substitution; traits are library declarations | R1280, R1466, R1287, R1461, and the hash and `__int__` leaves (steps 1–3) | about 20 (below) | yes, the largest |
| G. A capture-list closure is a capture list | a capture-list closure is a `__storage` struct of its captures, and its body a method of it | R1436; R1433 → R176 → R1434 | R176, R1349 | yes |
| H. Three compile-time evaluators | sema checks each expression once into a typed parameter expression; the elaborator alone evaluates it | R1474, R1475, R1480, R1482 | about 25 (below) | partial |
| I. Packs are binder kinds | a pack is a `!kgen.param_list` value whose library members CTFE applies | R1491, R1492, R1485, R1486 | about 20 (below) | slight |

### D′. Memory-only values live in registers

- **Upstream** (`UncheckedCallEmission.cpp`): a memory-only call result is
  written once into its destination or a synthesized
  `__call_result_tmp__`, and each consumer borrows or takes that storage.
  Only register-passable values are SSA registers (`CheckLifetimes`).
- **Mojito**: R1355 and R1356 moved most consumers onto the call-result
  slot. The shapes left are R1440–R1444, with probes on R1435 and R1095,
  and R1445 then rejects the class in the verifier.
- **Still open from R1355's list**: R1331 and R123 (the `$arg_loan_r`
  anchor), R715 (a `^` argument), R1212 (`$mat_r`), and R406 (a forwarded
  call-result subscript). Re-probe each.
- **Shrinks** R691, R1234, R118, R54, R1221, and R718.
- R473 stays open: its discard still copies, and a move into storage that
  dies unread runs.
- A raising call still hands its callee's error back in a register
  (R1438).

### F. Members reached by name, not called as the requirement

- **Mojito**: the backends dispatch display, the hash leaves, a scalar
  construction's `__int__`, and a `Variant` handler's `__call__` from the
  value's type, through `mir::TYPE_REACHED_MEMBERS`. Each such member
  keeps every argument, so it keeps one instance per `n`.
- **Upstream**: the traits are library declarations with default bodies,
  and every operator and protocol step is a `kgen.call` of the
  requirement, which substitution folds to the member.
- **Size**: R987 counts 868 `List.__len__` and 523 `Span.__len__`
  instances reached through trait-bound calls. R1280's iteration step
  cannot inline.
- **Closes**: R1280, R1466, R712, R579.
- **Shrinks** R987, R1291, R113, R1119, R25, R817, and R860.

### G. A capture-list closure is a capture list

- **Mojito**: every capture-list closure builds the pin's `__storage`
  struct, its body a method of it, and its value is the struct, a runtime
  value only. The check still names the struct by its site, MIR resolves
  it (R1436), and run-time `capturing[...]` values convert it to a
  closure of its body (R1433, R176, R1434).
- **Upstream**: a `F: def(...)` binder is instantiated at the struct
  type, so every call is direct. `kgen.create_closure` is made only for
  capture-free values.
- **Size**: an indirect call becomes direct, inlinable, and promotable.
- **Members**: R176 closes or shrinks. R303 is the inliner's.
  R1349 is independent. The generic-closure entries (R71, R83, R452) are
  about naming parametric functions, not closure representation.

### H. Three compile-time evaluators

- **Mojito**: `mojito-comptime`'s evaluator is deleted (R1388). The
  check still folds in two places: `compile_dependent_ct_expr`
  (`checker/constraints.rs`) with its adapters, and the lifted fallback
  (`Checker::lifted_application`).
- **Upstream** checks each expression once into a typed parameter
  expression. One elaborator evaluates it by CTFE, over any result type.
- **Members**, filed before R1388, to re-probe:
  - Former `Elab` arms: R1171, R1178, R1208.
  - `Self` and associated reads: R513, R436, R415.
  - Checker arms: R1199, R332, R1220, R1210, R448.
  - Domain: R480, R893, R396, R901, R481, R1197.
  - Constructions: R1175.
  - Spelling: R388 and R398.
  - Library-side folds: R231, R197, and R198.
- Gap H also unblocks most of the reflection block below.

### I. Packs are binder kinds

- **Mojito**: `*Ts` and `*values` are scoped binders, read at run time
  as the `TypeList` / `ParameterList` value the pin types them as. A
  compile-time `len`, subscript, and the `TypeList` vocabulary are still
  answered by spelling (`pack_length_binder`, `typelist_receiver`,
  R1284). A `__generator_type` parameter takes neither an unbound
  application (R1485) nor a type-valued generator (R1486).
- **Upstream**: a pack is a `TypeList` / `ParameterList` value, and its
  library members run by CTFE.
- **Cores**: R1491 and R1492 (a value pack, then a type pack, declares
  upstream's two binders: the hidden `vs.values` list and the visible
  `ParameterList` / `TypeList`), R1485, and R1486.
- **Members**:
  - Queries: R1284, R800, R904, R1182, R1302, R445, R803, R345, R804,
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
| Scalars, `SIMD`, `Bool`, `DType`, `StringLiteral`, and enum storage are compiler types | steps 1–3 (R1398, R1399, R1557 → R1558, R1608 → R1612), R160, R276, R1192 | R1408, R331, R850, R212, R1019, R423, R1352, R851, R187, R171, R330, R1189, R427, R432 |
| `Writable`, `repr`, and `print` are builtins each backend re-implements | R422 → R1287 | R1461, R1462, R1463, R1409, R1611, R275, R18, R159, R937 |
| Reflection is a builtin, not `Reflected[T]` | R324 | R271, R497, R272, R76, R77, R444, R273, R900, R904, R1481 |
| A trait requirement has no checked signature | R758, R594 | R1407 (after R942, R549, R571), R595, R769, R640, R760, R549, R942, R1165, R67, R571, R284, R550, R570 |
| The checked handoff is rebuilt from span-keyed facts | R764 → R765 → R766 → R767 | R756, R757, R755, R626, R627, R754, R768 |
| Origins are dropped from `ParamDecl` | R761 → R762 → R763 | R1300, R1299, R1213, R674, R133, R353, R104, R242, R726 |
| Loans come from call shape, not type-carried origins | R87 | R88, R93, R122, R126, R304, R355, R1323, R94, R89 |
| Overload identity is a partial mangled string | R600, R934 → R935 | R1172, R105, R64, R1294, R570, R1111 |
| Relocations name no `__moveinit__` | R1047, R341 | R731, R738, R732, R739, R740, R32, R1048 |
| One-shot parameter inference | R52 | R53, R537, R726, R61 |
| `Tuple` is `__RuntimeTuple` storage | R341, R359 | R343, R1121, R208, R1122, R1124, R207 |

The landed 10-25 blocks left one or two entries each, none with a core
to wait on:
- Type versus value: R383, R490.
- Conformance evidence: R792 (`conforms_to` takes one name and one
  trait), R1412.
- One lvalue: R1224. Transfers: R750. Abandonment: R302.

## Entries outside the gaps

None of these unblocks another entry. Run them whenever they are useful.

- **Inlining reach**: R1346, R1347, R1350, R1339, R1340, R1341, and R1452
  (the pin measures a callee before cleaning what it spliced in).
- **Bugs**: R1112, R1035, R933, R1289.
- **Hygiene**: R944, R868, R1276.
- **Library ports**: R1410 (`String`'s variadic constructor), R1411
  (`std.iter.map`). **Diagnostics**: R1413 (enum declaration errors).
- **Performance**: R896, R897. **Pin legality**: R924.
  **Canonicalizer completeness**: R1255, R1256.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1346 --only R1347 --only R1350 \
  --only R1339 --only R1340 --only R1341 --only R1452 --only R1112 --only R1035 --only R933 --only R1289 -n 0
scripts/claude_loop.py --model opus --plan-fable-on-opus --only R944 --only R868 --only R1276 \
  --only R896 --only R897 --only R924 --only R1255 --only R1256 -n 0
scripts/claude_loop.py --model opus --only R1410 --only R1411 --only R1413 -n 0
```

## Measure

Corpus totals from `benchmarks/instantiation.tsv`, refreshed by
`scripts/instantiation-report`. The overnight gate says when it is stale.

| counter | 10-13 | 10-17 | 10-21 | 10-24 |
|---|---|---|---|---|
| concrete functions | 42,409 | 24,864 | 25,859 | 26,934 |
| concrete instructions | 892,391 | 402,905 | 386,579 | 401,799 |
| generators | 16,553 | 8,801 | 9,213 | 9,500 |
| generator instances | 26,868 | 13,804 | 13,989 | 14,619 |
| instances per generator | 1.62 | 1.57 | 1.52 | 1.54 |

- Compare runs with `scripts/instantiation-report --check`, over the
  fixtures both runs elaborate, not with the raw totals.
- The program is less than half its 10-13 size. Almost all of that came
  from dead code removal, inlining, and the unused-argument removal that
  landed 10-17.
- Instances per generator has barely moved: a generic body is still copied
  about as often as before. Gaps E, F, and G aim at that ratio.
- The library `SIMD` (10-27/28) is real generic code and inlined operator
  chains. Refresh the table after the overnight gate, before step 2, to
  see what it cost.

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
