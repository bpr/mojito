# Ownership On A Compile-Time Region

The rule a `comptime if` and a `comptime for` follow in the ownership
analysis, found for roadmap entry "Nothing shows ownership can be decided on
a compile-time region" (2026-10-02, pin `Mojo 1.2.0.dev2026092105`,
`e9569894`). Stages P3a and P3b of
[`docs/parametric-mir-plan.md`](../parametric-mir-plan.md) give MIR a
structured compile-time conditional and loop; this note says what the
ownership analysis does with them.

## The rule

**A compile-time region is decided as the runtime region of the same shape,
with its condition opaque and its trip count unknown.** The join of a
`comptime if` is the join of an `if`: a value is initialized after the
region only if every arm leaves it initialized, and a value one arm consumes
is destroyed at the entry of every arm that does not. The body of a
`comptime for` is a loop body: a value consumed in it and not refilled is
consumed on the back edge, whatever the trip count, and a compile-time
`break` and `continue` are a loop's `break` and `continue`. An arm assumes
nothing from its condition; the body assumes nothing from its index. Return
and raise inside a region are what they are inside a runtime region: the
arm is unreachable after them, so the join reads the other arms.

Substitution then keeps the taken arm, or unrolls the body, with the
destroys the analysis placed. It may resolve a destructor witness (the
element of a heterogeneous pack destroys as its own type). It recomputes no
last use.

That is upstream's rule. `Mojo/lib/LowerLIT/CheckLifetimes.cpp` runs on
HLCF with `ComptimeIfOp` and `ComptimeForOp` still structured, before
parameter evaluation. Its `UninitializedValueScan::checkIfLikeOp` scans
every arm of a `ComptimeIfOp` from the region's entry liveness and
intersects their live-outs, exactly as for an elif-free `IfOp`;
`checkLoopOp` iterates a `ComptimeForOp` body to a fixed point of its
continue set, exactly as for `LoopOp`, with `ComptimeForBreakOp` and
`ComptimeForContinueOp` merged into the break and continue sets beside
`BreakOp` and `ContinueOp`. `DestructorInsertion::checkIfLikeOp` scans the
arms, unions their consume sets (`unifyConsumedSets`, which also propagates
the other arm's set over an unreachable arm), and
`destroyValuesAtEntryIfNeeded` destroys at each arm's entry what the union
demands and the arm does not consume; `DestructorInsertion::checkLoopOp`
dry-runs the body to a stable continue set and replays it once to insert
destructors. No compile-time region gets a rule of its own.

## The probes

`conformance/probes/comptime_region_*.mojo`, each a `def f[n: Int]()` over
a move-only `Thing` with a printing `__del__`. The pin's verdict and the
runtime-shaped twin's (the same program with `comptime if` spelled `if` and
`comptime for` spelled `for`) agree on every probe, output included, which is
the rule above observed from outside. `Mojito` is the production path, which
selects the arm and unrolls the loop before the check; `Keep` is the
experiment below.

| Probe | Shape | Pin | Runtime twin (pin) | Mojito | Keep |
| --- | --- | --- | --- | --- | --- |
| `c1` `move_taken_use_after` | taken arm consumes `a`; `a` used after the join | rejects: use of uninitialized `a` | same | rejects | rejects |
| `c2` `move_untaken_use_after` | untaken arm consumes `a`; `a` used after | rejects: use of uninitialized `a` | same | **runs** (`look a`, `look a`, `del a`) | rejects |
| `c3` `move_one_arm_no_use` | one arm consumes `a`, no use after; `b` used in both | runs: `before`, `consume a`, `del a`, `then b`, `del b`, `after`; for the other arm `before`, **`del a`**, `else b`, `del b`, `after` | same | runs, but `del a` precedes `before` in the else instantiation | same as pin |
| `c4` `untaken_arm_uses_moved` | `a` consumed before the region; untaken arm reads it | rejects | same | **runs** | rejects |
| `c5` `untaken_unused_decl_moves` | untaken arm declares `var t = a^` and never uses `t`; `a` used after | rejects | same | **runs** | rejects |
| `c6` `early_return` | taken arm returns; other arm consumes `b`; `b` used after | rejects: use of uninitialized `b` | same | rejects | rejects |
| `c7` `raise_in_arm` | taken arm consumes `a` and raises; `b` used after the join | runs: **`del b`** first (at the arm's entry), `consume a`, `del a`, `caught boom` | same | runs, `b` never destroyed | same as Mojito |
| `c8` `ref_bearing` | `ref r = a` before the region; `r` read in one arm and after | runs: `del b`, `then a`, `after a`, `del a`, `end` | same | same | same |
| `c9` `cond_use_then_after` | `a` read in one arm, `b` in the other, `a` after | runs: `del b`, `look a`, `look a`, `del a`, `end` | same | same | same |
| `c10` `both_arms_move` | both arms consume `a`; nothing after | runs | same | same | same |
| `l1` `zero_iter_move_use_after` | `range(0)` body consumes `a`; `a` used after | rejects | same | **runs** | rejects |
| `l2` `one_iter_move_use_after` | `range(1)` body consumes `a`; `a` used after | rejects | same | rejects | rejects |
| `l3` `one_iter_move_no_use` | `range(1)` body consumes `a`; nothing after | rejects, at the consume | same | **runs** | rejects |
| `l4` `per_iteration_value` | a `Thing` per iteration, read with `a` | runs: each `t<i>` destroyed after its read, `a` after the loop | same | same | same |
| `l5` `loop_carried_refill` | `a = rebuild(a^, i)` each iteration | runs | same | same | same |
| `l6` `break_continue` | `continue` at `i == 1`, consume `a` and `break` at `i == 3` | runs: `t1` destroyed at the `continue`, `t3` at the entry of the breaking arm | same | rejects: `'continue' outside of a loop` | same as Mojito |
| `l7` `break_after_move_use_after` | consume `a` then `break`; `a` used after | rejects | same | rejects (`'break' outside of a loop`) | same as Mojito |
| `l8` `pack_heterogeneous` | `def f[*Ts: Movable](var *args: *Ts)` over `A`, `B`, `A`; pack unused | runs: `del A y`, `del B 7`, `del A x` before the body, each as its own type; nothing for an empty pack | — | same | — |
| `l9` `pack_index_arms` | `comptime if i == 1` inside the pack loop | runs: pack destroyed at entry, `iter 0`, `skip 1`, `iter 2` | — | same | — |

Two findings beside the rule:

- Mojito's explicit-destruction analysis already follows it: `docs/features.md`
  records that an arm conditioned on a body parameter joins as an `if`
  branch there. The move analysis does not, because it runs after the arm
  is selected. The bold cells are the divergence; `c2`, `c4`, `c5`, `l1`,
  and `l3` are now `assets/extensions/ownership_ok/` fixtures on the roadmap
  ledger.
- `c7` is not about regions. A value live after a region and unused in the
  arm that raises is never destroyed when the raise leaves the function
  (`conformance/probes/raise_path_live_value_leaks.mojo`: the same leak
  follows a raising call with the value live after it, a raise in a loop,
  and a raise in an `else` arm; only a value dead at the raise is
  destroyed). It is a drop-elaboration defect on the roadmap.

## The experiment

`mojito run --comptime-regions keep` takes the probes through the whole
path with the rule in force, on today's representation:

1. The elaborator (`comptime::ComptimeRegions::Keep`) keeps every arm of a
   `comptime if` as a runtime `if` whose conditions are the evaluated
   literals, and a `comptime for` over a `range(...)` as a runtime `for`
   over the evaluated bounds, in the module that declares `main`. A region
   whose untaken arm or whose body does not elaborate under the
   instantiation (it reads a type fact the condition established, or
   indexes a pack with the loop variable) is selected as in production, and
   the bundled library's bodies always are, since their arms are typed by
   their conditions.
2. The checker, HIR, MIR, the ownership analysis, and drop elaboration see
   the region as the runtime region it now is. Nothing in them changed.
3. The elaborator (`native::mono::fold_literal_branches`) folds every
   branch whose condition is a literal its block defines into a jump to
   the taken successor, after drop elaboration, and the concrete graph is
   verified again. The untaken arm's blocks stay, unreachable. `--erased`
   runs the unfolded graph as the differential oracle.

Every probe's verdict then matches the pin's, and every accepted probe's
output matches the pin's byte for byte, except `c7` (the raise leak above,
present with and without the experiment) and `l6`/`l7`, where the checker's
symbolic validation of the template rejects a compile-time `break` and
`continue` before elaboration (`'continue' outside of a loop`), which the
`comptime for` entry of the roadmap covers. The fold changes no output: the
`--erased` run of each probe prints the same lines.

## What P3a and P3b take from this

- The structured compile-time conditional and loop lower to HIR as a branch
  and a loop whose condition is a parameter expression, not a value. The
  ownership analysis and drop elaboration need no new join rule: a
  parameter-expression condition is opaque to them, as a runtime value is.
- The elaborator evaluates the condition and keeps the taken arm after drop
  elaboration, as the fold does. The destroys at the untaken arm's entry
  go with it; the destroys at the taken arm's entry stay. A last use is
  never recomputed.
- A heterogeneous pack's body is one loop body whose element type is a
  parameter-expression type; the destroy the analysis places on an element
  resolves to the element's own destructor at substitution. The pin
  destroys an unused owned pack at the body's entry in reverse element
  order.
- Legality is decided once, on the template: an invalid untaken arm (`c4`)
  and an invalid unused declaration (`c5`) are rejected without an
  instantiation that takes them. A post-elaboration check would not see
  them, so it is not an acceptable substitute.
- Compile-time `break` and `continue` need the checker's symbolic
  validation to treat a `comptime for` body as a loop context before the
  MIR loop form can carry them.
