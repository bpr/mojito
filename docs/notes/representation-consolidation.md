# Shrinking Generic Code Early: Fix Representations, Not Shapes

The playbook for the `pmir` work, in answer to: *how do we move Mojito
closer to Mojo in as few steps as possible, with the goal of downsizing the
program representation as early as possible to prevent code explosion
during generic instantiation?* The next step is always at the top. Finished
steps are deleted; `git log -- docs/notes/representation-consolidation.md`
has them.

Entries are cited by stable ID and the order is computed, so every command
below stays valid as entries are filed and landed. Before a run, preview it
with `--dry-run` or `scripts/roadmap.py deps R890`. Every command runs on
Opus, with a plan pass for each entry whose `Model:` bullet says Fable.

## Where things stand (2026-10-21)

The region chain is finished: the post-elaboration pipeline (R881), the VM
(R882), and Pliron (R883) all read structured bodies, and since R1274
(10-22) MIR has no CFG terminator at all and `mir::verify` requires one
block per region. R1276 (a region typed as one block) is the cleanup that
remains, outside the gaps below.

The two leftovers this note listed both landed 10-22: R1266 (`sroa` and
`mem-2-reg` take generic slots) and R1270 (`cse` merges built-in
operators). So did gap A's last core entry, R884 (10-22): MIR lowering
copies each `finally` before every exit of its `try`, and each raise names
its `try` by label.

The 48 open `pmir` entries are not 48 problems. As in the pass that closed
the four representation gaps (10-16/17), most of them trace to a few places
where Mojito's representation differs from upstream's. Fix the
representation and the entries built on it shrink to a deletion or
disappear. Four such gaps were found, and one of them reaches far outside
`pmir`; gap A's core has landed, and its last column was re-checked
10-23.

## The four gaps

| gap | upstream's shape | core entries | entries that shrink or vanish |
|---|---|---|---|
| A. Raises are implicit | every raise point ends a region | none left (R906, R831, R20, and R884 landed) | R890 (R733, R833, R1129, R1106, R829, R1272, R1264, R1113, R1290, and R956 closed; R832 waits on gap D) |
| B. Construction and witnesses are not calls | a synthesized `__init__`; the witness folded at substitution | R1049, R1050, R1052, R988 (landed) | R1278, R1283 (closed), R1021 (closed), R1075 (closed), R1287 |
| C. Packs are built by the backends | the call builds a library pack value | R743, R1102, R1103, R1073 (landed) | R1304 (R1070 and R1090 closed) |
| D. Drops are decided after the fact | `DestructorInsertion` decides statically, per field | R1251, R931 | R1252, R932, R832, R1309 (R1267 closed) |

Each gap's fix also removes a place where a backend looks something up by
type instead of reading a name from MIR: the struct-named construction
(R1278; R1029 removed the by-arity lookup and R1283 the struct-named copy), the keyword collector at binding (R1070; R1102 had the call build the positional one), and the flag sweep at
return (R832); R1090 had each pack element's drop name its destructor. R1031 → R1109 is the one
remaining lookup of that kind outside the four gaps.

### A. Raises are implicit: the biggest lever

This gap blocks about 14 entries across four tracks (`pmir`,
`ownership`, `native`, `mojo-shape`), more than any other.

- **Upstream** (`lib/LowerLIT/LowerSemanticCF.cpp`): a `raise` statement is
  a terminator, and `addErrorRegions` puts an `hlcf.if` on the error flag
  after every raising call. Its then-region ends in `emitRaise`:
  `lit.try.raise` inside a `try`, `lit.error.return` out of the function.
  `lowerTryFinally` copies each `finally` before every exit. Every raise
  point is then an explicit region exit, and `CheckLifetimes` and
  `DestructorInsertion` destroy on each one what that exit holds.
- **Mojito**: a `raise` statement is the `MirTerm::Raise` terminator
  (R906 landed 2026-10-22), and since R831 (landed 2026-10-22) a raising
  instruction defines its error flag and error and is followed by its
  error region, an `if` on the flag ending in a `raise` (MIR text 1.91),
  so drop elaboration destroys on each exit what that exit holds, out of
  the function too (R20 closed with it), as upstream does. Since R884
  (landed 2026-10-22) MIR lowering copies each `finally` before every exit
  of its `try` (`mir/try_finally.rs`), each raise names its `try` by label
  (MIR text 1.92), and the `try` cleanup lists and `return.cleanup` are
  gone.
- **What follows** once raises are explicit exits:
  - R833 (drop order on a raise) closed with R831, and R1129, the same
    ledger row, with it. R733 (an outer value last used after the raise)
    matched the pin on both backends at the 10-23 re-check and closed;
    `assets/ok/try_outer_value_dies_on_raise_edge.mojo` pins it.
  - R956 closed 10-23: an initializer's `out self` is a drop root that
    every `return` reads, so it dies where a raise leaves it, whole once
    every field is set and field by field before, as
    `DestructorInsertion` destroys a partly built `self`. It filed R1295
    (upstream tracks each field of `self`) and R1296.
  - R1264 closed 10-23: a `try` has results and handler and `else`
    arguments, and `try.yield` and a labelled `raise` carry values, as
    `lit.try.raise` does, so `mem-2-reg` promotes a slot stored in a `try`
    part. It filed R1292 (the absent `except`/`else` stubs) and R1293 (the
    error as an operand rather than `$err`).
  - R1290 closed 10-23: a raising `__iter__` is an ordinary raising
    `call.method` with its error region, as `emitForStmt` calls it, so no
    raising call is left without a raise edge; natively, such a call is
    now unsupported rather than a jump that skips releasing temporaries.
  - R1106 closed with R831: the explicit destroy's consume precedes the
    raise edge, so a raising callee binds the part.
  - R1113 closed 10-23: `lower_raise` releases the temporaries of the
    blocks a raise leaves, as the other exits do; a tuple element computed
    before a raising call reproduced it.
  - R1272 closed 10-23: a `try` part is a VM cursor level, and its probe
    recurses 100 000 deep.
  - R890 closed 10-23: a `for` loop calls a raising `__next__` as a
    `call.method` with its error region inside a `try` whose handler runs
    the loop's `else` and breaks, and `TryNext` is gone.
  - R832 needs no flag for raise edges any more; its remaining flags go
    with gap D, so it now depends on R932.
  - R829 closed with R884: its probe matched the pin by then, and the
    return cleanup list it named is gone.
- **Cost**: generators grow by one `if` per raising call. That is
  upstream's shape. Expect the instruction counters to rise slightly.
- **Order**: the core is done (R906, R831, R20, and R884 landed); what
  remains is the last column, all independent.

### B. Construction and witnesses are not calls: the biggest size lever

This gap is the most direct attack on instances per generator. Each core
entry's probe is "one instance per `n` where the pin has one".

- R1049 synthesizes the fieldwise `__init__`, so `call { func: Fw }` stops
  reading `Fw[n]` whole.
- R1050 makes a copy constructor restate its struct's parameters, as
  `__init__` does.
- Together they left R1029 a deletion, and it landed: neither backend
  resolves a constructor by arity. A call naming a struct remains only for
  a fieldwise struct with a `ref` field (R1278).
  R1097 moved `T()`/`T(copy=x)` into the shared
  `mir::construct_type_parameters`, and R1283 (10-23) made its struct arm
  name the `Defaultable` witness and the `__copyinit__` member rather than
  the struct, closing R724's explicit `S(copy=s)` with it.
- R1052 records each conformance's witness table on the declaration, so
  substitution folds a trait dispatch to the member. It covers operators,
  subscripts, and iterator steps through a bound as well, not only
  `__len__`.
- After R1052, R988 (library `len`) closed R1021 with no code change
  (2026-10-22).
- R1075 then priced a `for` loop by the bodies of the `__iter__` and
  `__next__` it calls (2026-10-22). `print` keeps an observed price until
  it is the library function (R1287), and the loop's last gap waits on
  R890's `__next__` call in a `try` (R1286).

### C. Packs are built by the backends

- R1102 landed (2026-10-23): every call builds its positional collector
  (`tuple.make`) and passes it as one operand, the backends collect
  nothing, and `inline-parametric`'s `Binding::Collector` is gone.
- R1103 landed (2026-10-23): a spread passes the caller's own pack instead
  of rebuilding one of its elements; only the builtin `print` still takes
  the elements (R1287).
- R1070 applies the same fix to the keyword collector.
- R743 landed (2026-10-23): a homogeneous collector is the library's
  `VariadicList`.
- R1073 landed (2026-10-23): its field is upstream's `Span` of element
  pointers, which the call builds over the caller's places, and the
  native element code is library code. R1304 has the call run the list's
  constructor from a `!pop.array` instead of building the span itself.

### D. Drops are decided after the fact

- R1251 moves the drop-shape decision into a field-sensitive destructor
  walk, and R1252 then folds into it.
- R931 → R932 lets `lower-lit` erase every ownership marker, which needs
  drops to be decided statically.
- R832's native drop flags go with R932.
- R1309 waits on R932: once no backend tracks an owned temporary, a
  `field.get` of a forwarded struct moves its field out, as upstream's
  `kgen.struct.extract` does, and `sroa` drops its rule for a slot stored
  from a slot it keeps.
- Do gap D after gap A: upstream's walk destroys on each raise exit, and
  with gap A fixed this walk has those exits to work on.

## Recommended order

1. **Gap B**, for size. R1049, R1050, R1052, R1029, R1097, R988, and
   R1075 landed 10-22, and R1283 on 10-23. What remains is R1278, which
   waits on R1277 and R1082.

2. **Gap A**, for leverage. Its core has landed (R906, R831, R20, R884),
   and the 10-23 re-check closed R733 and reworded R1113, R832, and R890.
   Run what is left:

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --only R890 -n 0
   ```

   R832 waits for gap D.

3. **Gap C**. R1102, R1103, R743, R1070, and R1073 landed. What remains
   is R1304:

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1304 -n 0
   ```

4. **Gap D**. R1252 brings in R1251, and R932 brings in R931:

   ```sh
   scripts/claude_loop.py --model opus --plan-fable-on-opus --task R1252 --task R932 --task R832 --task R1309 -n 0
   ```

## Entries outside the gaps

None of these unblocks another entry. Run them whenever they are useful.

- **Generator size and inlining reach**, for the measure: R1096,
  R1098 (lets the third round inline through a known callee), R1099,
  R1105, R1076, R1110, R1268. R1271 (`AutomaticInline`) is the largest
  but shrinks only concrete code, not instance counts.

  ```sh
  scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1096 --only R1098 --only R1099 \
    --only R1105 --only R1076 --only R1110 --only R1268 --only R1271 -n 0
  ```

- **Bugs**: R1112 (an elaboration failure the pin runs), R983 (hidden by
  the pre-elaboration pipeline), R1035, R933, and R1289 (a closed
  `comptime if`'s dead arm keeps values alive, filed by R906).

  ```sh
  scripts/claude_loop.py --model opus --plan-fable-on-opus --only R1112 --only R983 --only R1035 --only R933 --only R1289 -n 0
  ```

- **Hygiene**: R934 → R935 (typed symbol references), R944, R868, and
  R1031 → R1109 (the last backend lookup by type outside the gaps).

  ```sh
  scripts/claude_loop.py --model opus --plan-fable-on-opus --task R935 --task R1109 --task R944 --task R868 -n 0
  ```

- **Performance**: R896, R897. **Pin legality**: R924. **Canonicalizer
  completeness**: R1255, R1256.

  ```sh
  scripts/claude_loop.py --model opus --plan-fable-on-opus --only R896 --only R897 --only R924 --only R1255 --only R1256 -n 0
  ```

## Measure

Corpus totals from `benchmarks/instantiation.tsv` (refreshed by
`scripts/instantiation-report`; the overnight gate says when it is stale):

| counter | 10-13 | 10-17 | 10-21 |
|---|---|---|---|
| concrete functions | 42,409 | 24,864 | 25,859 |
| concrete instructions | 892,391 | 402,905 | 386,579 |
| generators | 16,553 | 8,801 | 9,213 |
| generator instances | 26,868 | 13,804 | 13,989 |
| instances per generator | 1.62 | 1.57 | 1.52 |

The 10-21 column is after `sroa` (R928), over 97 more fixtures than
10-17's. Over the fixtures elaborated both before and after it, `sroa`
took concrete instructions from 388,659 to 386,462 (−0.57 %), generator
instances from 14,037 to 13,988 (−0.35 %), and generator-instance
instructions from 158,373 to 157,799 (−0.36 %).

The program is less than half its 10-13 size, almost all of it from dead
code removal, inlining, and the unused-argument removal that landed 10-17.
Instances per generator barely moved: a generic body is still copied about
as often as before. Gap B is the entry set aimed at that ratio; R1266
(10-22) took generator instances from 14,003 to 13,996.
Refresh the table after each step above and watch it.

## Landed so far

The four representation gaps are closed (10-16/17): nested `def` calls are
direct (R1037), defaults are filled at call binding (R1077, R1060),
lifecycle operations are synthesized calls (R1041–R1046), and
`inline-parametric` is one substitution over every binder kind
(R1078–R1081). The region chain (10-20/21) moved the ownership checks,
drop elaboration, the pre-elaboration pipeline, the elaborator, the
post-elaboration pipeline, the VM, and Pliron onto `hlcf` regions with
values (R875–R883, R886, R1258–R1260). `Mem2Reg` now carries slots through
them (R927), `SROA` landed (R928) and takes generic slots (R1266), and
`cse` ends the fourth round and merges reads and trivial values (R926,
R929, R930). Unused-argument removal (R974, R975, R1032–R1034, R1039)
followed. What those left behind sits in
the `pmir` track (`scripts/roadmap.py list --track pmir`): one entry per
cause, not per shape, as the loop prompt now asks.
