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
Opus, with a plan pass for each entry whose `Model:` bullet says Fable.
`--task` runs an entry after its open prerequisites. `--only` runs exactly
the entries named. A single command cannot use both.

## Recommended order

1. **Finish gap D** (size). R1356 landed in part on 10-26: register
   passability judges a conditional conformance at closed arguments, and
   a memory-only slice literal lives in its call-result slot. Its census
   filed the shapes that still destroy a memory-only register as R1440
   (reference results copied for a `read` parameter), R1441 (builtin
   arguments), R1442 (operands off the call-result path), R1443 (copied
   consuming receivers), and R1444 (callable binder copies), with probes
   on R1435 and R1095.

   R1445, the verifier rule, comes last, behind all of them.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1445 -n 0
   ```

2. **Sweep the stale entries** (leverage, cheap). R1400–R1406 cover 99
   open entries, one sweep per track:
   - 17 entries are already fixed, partly fixed, or meet their own done
     condition, by probes and `rg` checks run on 10-25.
   - The rest name code no longer in `crates/`, wait on an entry that has
     landed, or restate another entry.
   - Each sweep re-runs the probes, deletes what matches the pin, rewrites
     the rest against today's code, merges duplicates, and adds the
     `Depends` links the 10-25 cores need.

   This shrinks every cluster below before any of them is planned.

   ```sh
   scripts/claude_loop.py --model opus --only R1400 --only R1401 --only R1402 --only R1403 \
     --only R1404 --only R1405 --only R1406 -n 0
   ```

3. **Gap E: compile-time values are slots** (size, small).

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R663 -n 0
   ```

   R662 landed (2026-10-26): a non-callable value binder's read is its
   `kgen.param.constant`. A callable binder's read is still a slot
   (R1449, behind gap G's R533).

4. **The inliner's `try` veto** (size). R1288 and R1286 landed
   (2026-10-26): a callee holding a `try` inlines, and a loop's `__iter__`
   is an ordinary call the inliner splices, so loop-bearing
   `@always_inline` bodies inline below the bound and R1280 (gap F) is
   reachable. The pin keeps some that inline here, because it measures a
   callee before cleaning what it spliced in (R1452).

5. **Gap F: members reached by name, not called** (size, the largest
   lever left, about 30 entries). It lands in three stages.

   First, the bundled library declares the compiler-known traits, so every
   conformance has a witness table. The entries are R1360 (`Boolable`,
   `Intable`, `Floatable`, `Indexer`, `Identifiable`), R1361 (the
   `std.math` traits), R1362 (`Hashable`, `Hasher`), R1363
   (`Defaultable`, `RegisterPassable`, `TrivialRegisterPassable`), R705
   (`Equatable`, `Comparable`), R424 (`Writable`), and R578 (the bundled
   types declare what upstream's do):

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1360 --only R1361 --only R1362 \
     --only R1363 --only R705 --only R424 --only R578 -n 0
   ```

   Second, operators become calls of the requirement. R1357 has the checker
   check `a op b` as `a.__op__(b)`. R1358 does the same for an operator over
   a bound, and R1359 does it for tuple comparison. R1348 makes a struct
   operator a call in MIR:

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1358 --task R1359 --task R1348 -n 0
   ```

   Third, R1281 reads the witness table instead of composing a name. R1109
   and R1031 then become deletions:

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1281 --task R1109 -n 0
   ```

   The lifecycle traits (`Copyable`, `Movable`, `Deinitable`, …) are not
   filed. The checker's copy and move synthesis is tied to them, and
   upstream declares them in `std/traits/`.

6. **Gap G: a capture-list closure is a capture list, not a struct**
   (size).
   - R1376 and R1377 landed: every capture-list `def` builds the pin's
     `__storage` struct at its declaration, its lifted body takes the
     struct as `self`, and every call passes it.
   - R1427 landed: every capturing lambda builds its storage where it
     stands, an immediately invoked one calls the body passing it, and
     one used as a value moves it into its closure. R1428 makes the body
     a real method of the struct.
   - R1378 landed: the closure's value is the struct, a runtime value
     only, which the check names by its site and MIR resolves (R1436
     moves the struct into the check). Run-time `capturing[...]` values
     still convert it to a closure of its body until R1433, R176, and
     R1434 withdraw them.
   - Legacy `@parameter` closures keep upstream's environment-pointer path,
     which R533 finishes.
   - R1344 and R1345 serve capture-free and `@parameter` closures.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R533 --only R1344 --only R1345 -n 0
   ```

7. **Gap H: three compile-time evaluators** (the largest block, about 45
   entries).
   - The size lever is partial: R389 stops equal applications splitting
     instance keys, and R893 stops an instance-budget blowup.
   - The chain R1379–R1388 is `docs/parametric-mir-plan.md` P4's unfinished
     remainder. It moves each kind of site off `Elab::eval` and onto the
     check plus `native::mono`, then deletes `Elab`'s evaluator and type
     resolver (R1388).
   - R1385–R1387 give the checker one folder for bracket arguments,
     associated members, and type operands.

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R389 --only R849 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1388 --task R1387 -n 0
   ```

8. **Gap I: packs are binder kinds** (slight size, about 20 entries):

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1169 --only R1151 --only R808 --only R1342 -n 0
   ```

9. **The conformance blocks**. These do not shrink generic code. Each is
   one cause behind 5 to 30 entries, listed under *Blocks without a size
   lever*. The four filed on 10-25 are:
   - A library `SIMD` as the one scalar type: R1389–R1399, with R1397 the
     port itself.
   - Type versus value decided by what a name denotes: R1369–R1372.
   - One subscript lvalue: R1373–R1375.
   - The checker's conformance evidence (R1364, R1365), transfers (R1366,
     R1367), and abandonment (R1368).

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1368 --task R1367 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1371 --task R1372 --task R1375 --task R1374 -n 0
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1398 --task R1399 -n 0
   ```

## Where things stand (2026-10-25)

The first pass's four gaps (10-21 to 10-24) are closed except for three
entries.
- Gap A (raises are explicit region exits) has fully landed, with R890
  last.
- Gap C (the call builds the library pack value) has fully landed, with
  R1304 last.
- Gap B has fully landed, with R1338 last. Its residues are conformance
  and cleanup: R1414 (a display's elements bind as call arguments), R695
  after it, R1415 (fieldwise keywords), and R1416 (hand-written MIR).
- Gap D has R1355 and R1356 left, and R1355 turns out to be the core of a
  larger cluster (gap D′).

To find the next gaps, the whole roadmap was read on 10-25, track by track,
for entries sharing one cause. The explicit `Depends` graph shows almost
nothing: the largest transitive fan-out of any open entry is 5. The
leverage is in shared causes that the entries name in prose but do not
link. Five new gaps (E–I) affect generic code size. Another dozen clusters
are conformance only.

`enum` landed after that reading (10-25) and moves no gap:
- Each enum is an ordinary struct. `mojito_parser::enums` generates its
  members as Mojo text, so they elaborate and instantiate like any
  hand-written struct's.
- Its storage, `__EnumStorage[*Ts]`, is the `TyKind::Variant` intrinsic
  `Variant` already used, selected by case index.
- It closed R1236. Its seven residues joined existing blocks: R1407,
  R1408, R1409, and R1412 (see *Blocks without a size lever*). R1410,
  R1411, and R1413 are under *Entries outside the gaps*.
- R1407 also waits on gap G's R1378 and the `Tuple` block's R1124.

| gap | upstream's shape | core entries | entries that close or shrink | size lever |
|---|---|---|---|---|
| D′. Memory-only values live in registers | `__call_result_tmp__` storage, register-passable SSA only | R1355, then R1440–R1444, then R1445 | about 25 (below) | yes |
| E. Compile-time values are slots | `kgen.param.constant` of the decl ref, no storage | R662, R663 (landed), R1449, R1451 | R788, R378, R472, R508, R602, R1194 | yes |
| F. Members reached by name from the value's type | every operator and protocol step is a call of the requirement, folded at substitution; traits are library declarations | R1360–R1363, R705, R424, R578; R1357 → R1358, R1359; R1348; R1281 | about 30 (below) | yes, the largest |
| G. A capture-list closure is a capture list | a capture-list closure is a `__storage` struct of its captures, and its body a method of it | R1376, R1377, R1427, R1428, R1378 (landed) → R1436; R1433 → R176 → R1434; R533, R1344 → R1345 for the other closure forms | R1104, R100, R1230, R1333 (closed), R534, R176 | yes |
| H. Three compile-time evaluators | sema checks each expression once into a typed parameter expression; the elaborator alone evaluates it | R386, R389, R849, R603; R1379–R1388 | about 45 (below) | partial |
| I. Packs are binder kinds | a pack is a `!kgen.param_list` value whose library members CTFE applies | R1169, R1151, R808 | about 20 (below) | slight |

### D′. Memory-only values live in registers

- **Upstream** (`UncheckedCallEmission.cpp`): a memory-only call result is
  written once into its destination or a synthesized
  `__call_result_tmp__`, and each consumer borrows or takes that storage.
  Only register-passable values are SSA registers (`CheckLifetimes`).
- **Mojito**: the result stays in a register, and each consumer binds its
  own hidden slot through `Node::is_fresh_temporary`. The slots are
  `$tmp_arg_r`, `$arg_loan_r`, `$mat_r`, `$ptr_tmp_r`, `$fwd_arg_r`,
  `$tmp_recv_r`, `$tmp_field_r`, `$discard_r`, and the checker's
  `materialize_temporary_holder`. Each has its own lifetime.
- **R1355 closes**:
  - R1331 and R123: the `$arg_loan_r` anchor outlives its call or destroys
    twice.
  - R715: a `^` argument gets no slot.
  - R1212: `$mat_r` drops the owner loan `$ptr_tmp_r` keeps.
  - R406: a forwarded call-result subscript has no place.
  - R716: a t-string temporary dies early. Re-probe it after R1328 and
    R1336.
- **R1355 shrinks** R691, R1234, R118, R54, R1221, R718, and R121's
  second probe.
- **R136** landed 10-25: an implicit copy at its source's last use
  forwards or moves, and R827 closed with it. R473 stays open: its
  discard still copies, and a move into storage that dies unread runs.
- **R1293** landed 10-25: a `try`'s raises store their error into its
  error slot, which dies as a variable.
- **R1437** landed 10-26: a raise out of a function stores its error into
  the function's error slot, which dies as a variable too. A raising call
  still hands its callee's error back in a register (R1438).
- **R1356** landed in part 10-26 (see *Recommended order*): **R1445 then
  rejects the class**, behind R1440–R1444, R1435, and R1095.

### F. Members reached by name, not called as the requirement

The struct-named construction of gap B is gone. This is the larger lookup
by type that remains. It also accounts for most of the inlining reach still
missing.

- **Mojito** reaches these members by name, not through a call of the
  requirement:
  - MIR lowers a struct operator as a `binary` naming its dunder, a
    multi-subscript as `index.multi` or `slice` naming an accessor, and an
    iteration step as `__trait_dispatch.*` / `__iterator_dispatch.*`
    names.
  - The elaborator composes `{struct}.{method}` (`resolve_method_symbol`,
    `Instances::dispatched_overload_target`).
  - The backends dispatch from the value's type through
    `mir::DISPATCHED_MEMBERS`, `PROTOCOL_MEMBERS`, and
    `SUBSCRIPT_ACCESSORS`.
  - R1052 folded the witness only for traits that have a declaration. The
    rest of `mojito_types::conformance::COMPILER_TRAITS` records no
    witness table.
  - The checker types operators apart from method calls (`infer_infix`,
    `infer_prefix`, `bound_comparison_target`, the structural
    `tuple_order_compatible`).
- **Upstream**: the traits are library declarations with default bodies.
  `a op b` is checked as `a.__op__(b)` and emitted as a `kgen.call` of
  the requirement, which substitution folds to the member.
- **Size**:
  - R1109 is "one instance per `n`" for every table member.
  - R987 counts 868 `List.__len__` and 523 `Span.__len__` instances
    reached through trait-bound calls.
  - R1348's operators and R1280's iteration step cannot inline.
- **Closes**:
  - Dispatch: R1109, R1031, R1280.
  - Operators: R708, R1332, R1282, R81, R697, R1238.
  - Traits: R707, R706 (after R705), R109 (after R424), R712, R579, R191.
- **Shrinks** R987, R1291, R684, R113, R1119, R25, R817, R862, R860,
  R861, and R863.

### G. A capture-list closure is a capture list

- **Mojito**: `closure.make` keeps a closure's captures as a list, and
  `call.indirect`'s `callee_place` names a slot. Both closure forms share
  that representation: capture-list nested `def`s and legacy `@parameter`
  closures.
- **Upstream** treats the two forms differently:
  - A capture-list closure is a `<fn>::__storage` struct, one field per
    capture, built by `ClosureEmitter::emitClosure`. `liftClosureIntoMethod`
    makes its body a method taking the struct. A `F: def(...)` binder is
    instantiated at the struct type, so every call is direct.
  - A legacy `@parameter` closure is keyed on its body, and its
    environment goes in as trailing pointer arguments. That is Mojito's
    environment path, which R533 makes the only one.
  - `kgen.create_closure` is made only for capture-free values, the form
    `CallIndirectOp::canonicalize` folds (R1344, R1345).
- **Size**: an indirect call becomes direct, inlinable, and promotable.
- **Cores**:
  - R1376 builds the storage struct.
  - R1377 makes the body a method taking it.
  - R1378 (landed) types the closure as the struct, a runtime value only.
    The pin rejects a capturing capture-list closure as a compile-time
    argument.
- **Members**:
  - R1104 closes. R100, R534, and R176 close or shrink.
  - R983, R980, R172, and R303 follow R533, not the struct.
  - R1349 and R1018 are independent.
  - The generic-closure entries (R71, R83, R452, R498) are about naming
    parametric functions, not closure representation.

### H. Three compile-time evaluators

- **Mojito** evaluates compile-time expressions above MIR in three places,
  each with one arm per expression shape:
  - `mojito-comptime`'s `Elab`: `eval`, `type_from_name`,
    `associated_value_at`, `apply_generic_alias`, and the `TypeAliases`
    table. It has its own name resolution and no `Self`.
  - The checker's `eval_associated_ct`, `compile_dependent_ct_expr`, and
    `constraint_operand` (`checker/constraints.rs`), and
    `comptime_type_operand` and `member_type_operand`.
  - The lifted fallback (`Checker::lifted_application`). It serves only
    `Int` and `Bool` results, and mints a `$comptime$$N` function per
    occurrence rather than per expression.
- **Upstream** checks each expression once into a typed parameter
  expression. One elaborator evaluates it by CTFE, over any result type.
- **Core chain**, `docs/parametric-mir-plan.md` P4's remainder, one kind
  of site per entry:
  - R1379: local value bindings.
  - R1380: local type aliases.
  - R1381: module constants.
  - R1382: module and generic aliases.
  - R1383: reflection.
  - R1384: struct-valued bracket arguments.
  - R1385: associated members that apply a callable.
  - R1386: the checker's second per-shape folder.
  - R1387: type operands.
  - R1388: delete what is left of `Elab`'s evaluator and type resolver.
- **Slices filed earlier**:
  - R386: a value meta for any result type.
  - R389: one lifted function per expression, so equal applications are
    one type (R380, R390).
  - R849: every condition is decided by `param_expr::fold`.
  - R603: one static receiver for `Self.X`, `Alias.X`, and `Type.X`.
- **Members**:
  - `Elab` arms: R1245, R475, R1170, R1206, R1171, R518, R489, R802,
    R1178, R1208.
  - `Self` and associated reads: R269, R513, R338, R436, R415.
  - Checker arms: R1194, R1191, R1199, R476, R1180, R461, R332, R462,
    R1220, R1210, R448, R512.
  - Domain: R480, R893, R396, R397, R901, R481, R1197.
  - Constructions: R135, R853, R1179, R1175.
  - Spelling: R388 and R398.
  - Library-side folds: R217, R231, R197, and R198.
- Gap H also unblocks most of the reflection block below.

### I. Packs are binder kinds

- **Mojito**: `*Ts` and `*values` are scoped binders. `len`, subscript,
  and the `TypeList` vocabulary are answered by spelling (`infer_len`,
  `pack_length_binder`, `typelist_receiver`,
  `comptime/rewrite.rs:fold_pack_typelist_use`), and there is no
  `__generator_type` parameter kind.
- **Upstream**: a pack is a `TypeList` / `ParameterList` value, and its
  library members run by CTFE.
- **Cores**: R1169 (a type pack is a `TypeList` value), R1151 (a value
  pack is typed as the raw list), and R808 (generator parameters).
- **Members**:
  - Queries: R1284, R800, R904, R1182, R1302, R445, R803, R345, R804,
    R348, R1071.
  - Library ports: R810, R811, R812, R1181, R223.
  - Pack constraints: R1342 and R1343. R1342 is the size entry: a concrete
    pack query keeps its generator.

### Blocks without a size lever

Each is one cause, by roadmap reading, listed by number of members. They
move Mojito toward Mojo's shape and delete compiler special cases. Some
may raise instance counts: a library `SIMD` is real generic code.

| block | core | members |
|---|---|---|
| Scalars, `SIMD`, `Bool`, `DType`, `StringLiteral`, and enum storage are compiler types | R1389–R1399 (`Int`, `UInt`, and `Float64` become width-one `SIMD`, then the library `SIMD` port, R1397), R160, R1188, R276, R1192 | R1408 (after R1392), R143, R339, R335, R331, R430, R850, R212, R1019, R423, R1352, R186, R185, R851, R187, R171, R330, R336, R857, R1189, R427, R432 |
| `Writable`, `repr`, and `print` are builtins each backend re-implements | R424 → R422 → R1287 (R424 is also gap F) | R1409, R433, R434, R275, R18, R159, R937 |
| Reflection is a builtin, not `Reflected[T]` | R324 (after gap H) | R271, R497, R272, R76, R569, R270, R77, R78, R79, R75, R443, R444, R273, R900, R904 |
| A trait requirement has no checked signature | R758, R594 | R1407 (after R942, R549, R571), R595, R769, R640, R760, R549, R942, R419, R1165, R67, R571, R284, R550, R570 |
| The checked handoff is rebuilt from span-keyed facts | R764 → R765 → R766 → R767 | R756, R757, R755, R626, R627, R754, R768, R521 |
| The parser decides type versus value | R1369 → R1370 → R1371, R1372 | R377, R381, R512, R383, R482, R1203, R72, R326, R490, R134 |
| Origins are dropped from `ParamDecl` | R761 → R762 → R763 | R1300, R1299, R1213, R674, R133, R353, R104, R242, R726 |
| Loans come from call shape, not type-carried origins | R87 | R88, R93, R122, R126, R304, R355, R1323, R94, R89 |
| Overload identity is a partial mangled string | R600, R934 → R935 | R1172, R105, R64, R1294, R570, R1111, R91 |
| Relocations name no `__moveinit__` | R1047, R341 | R731, R738, R732, R739, R740, R32, R1048 |
| One-shot parameter inference | R52 | R53, R479, R382, R537, R726, R61 |
| No evidence environment for conformance | R1364 → R1365 | R667, R792, R704, R128, R698, R1412 |
| No single lvalue | R1373 → R1374, R1375 | R35, R690, R1247, R1224, R1233 |
| A transfer's source is judged case by case | R1366 → R1367 | R31, R116, R120, R121, R750 |
| Abandonment is the `explicit_destroy` walk, not `Deinitable` | R1368 (after R1364, R1365) | R30, R302, R711, R667 |
| `Tuple` is `__RuntimeTuple` storage | R341, R359 | R343, R1121, R208, R1122, R1124, R207 |

### Stale entries

The sweep entries hold the lists, each with its reason:
- R1400 and R1401: `comptime`.
- R1402: `mojo-shape`.
- R1403: `divergences`, `pmir`, `native`, `code-org`, and `tooling`.
- R1404: `calls`.
- R1405: `stdlib`.
- R1406: `ownership`.

The sweeps also add the members' `Depends` links to the 10-25 cores:
- R667 and R129 on R1364.
- R1224 on R1374.
- R336 on R1392 and R1395.
- R276 on R1397.
- R983, R172, and R303 on R533.

## Entries outside the gaps

None of these unblocks another entry. Run them whenever they are useful.

- **Inlining reach**: R1346, R1347, R1350, R1339, R1340, R1341.
- **Bugs**: R1112, R1035, R933, R1289.
- **Hygiene**: R944, R868, R1276.
- **Library ports**: R1410 (`String`'s variadic constructor), R1411
  (`std.iter.map`). **Diagnostics**: R1413 (enum declaration errors).
- **Performance**: R896, R897. **Pin legality**: R924.
  **Canonicalizer completeness**: R1255, R1256.

```sh
scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1346 --only R1347 --only R1350 \
  --only R1339 --only R1340 --only R1341 --only R1112 --only R1035 --only R933 --only R1289 -n 0
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

- The 10-24 baseline covers 1,200 fixtures, 31 more than 10-22's. Over
  the same period, gap A gave every raising call an error region, as
  expected. Compare runs with `scripts/instantiation-report --check`, over
  the fixtures both runs elaborate, not with the raw totals.
- The program is less than half its 10-13 size. Almost all of that came
  from dead code removal, inlining, and the unused-argument removal that
  landed 10-17.
- Instances per generator has barely moved: a generic body is still copied
  about as often as before. Gaps E, F, and G aim at that ratio. Gap F's
  R1109 is the most direct attack on it.
- Refresh the table after each step above.

## Landed so far

The first four representation gaps (10-16/17):
- nested `def` calls are direct (R1037);
- defaults are filled at call binding (R1077, R1060);
- lifecycle operations are synthesized calls (R1041–R1046);
- `inline-parametric` is one substitution over every binder kind
  (R1078–R1081).

The region chain (10-20/21) moved these onto `hlcf` regions with values
(R875–R883, R886, R1258–R1260): the ownership checks, drop elaboration, the
pre-elaboration pipeline, the elaborator, the post-elaboration pipeline,
the VM, and Pliron. Since R1274, MIR has no CFG terminator. In the same
period:
- `Mem2Reg` carries slots through regions (R927);
- `SROA` landed (R928) and takes generic slots (R1266);
- `cse` merges reads, trivial values, and built-in operators (R926, R929,
  R930, R1270);
- unused-argument removal followed (R974, R975, R1032–R1034, R1039).

The second pass (10-21 to 10-24) landed:
- gap A, every raise point an explicit region exit (R906, R831, R20, R884,
  R890, and its last column);
- gap C, where the call builds the library pack value (R1102, R1103, R743,
  R1070, R1073, R1304);
- gap B (R1049, R1050, R1052, R1029, R1097, R988, R1075,
  R1283, R1278, and on 10-25 R1338);
- gap D except R1445 and its prerequisites (R1251, R1252, R931, R932,
  R1306, R832, R1310–R1313, R1309, R1319, R1320, R1325, R1329, R1327,
  R1336, R1321, R1330, R1326, R1337, R1328, R1355, and R1356).
