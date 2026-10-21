# Shrinking Generic Code Early: Fix Representations, Not Shapes

The playbook for the `pmir` work, in answer to: *how do we move Mojito
closer to Mojo in as few steps as possible, with the goal of downsizing the
program representation as early as possible to prevent code explosion
during generic instantiation?* The next step is always at the top. Finished
steps are deleted; `git log -- docs/notes/representation-consolidation.md`
has them.

Entries are cited by stable ID and the order is computed, so every command
below stays valid as entries are filed and landed. Before a run, preview it
with `--dry-run` or `scripts/roadmap.py deps R927`. Add `--model opus
--plan-fable-on-opus` to any command when Fable usage runs short.

## Next: structured regions, then `Mem2Reg` through them (2026-10-20)

Upstream's lifetime checks, drop elaboration, and `Mem2Reg` all walk
`hlcf` structured regions. Mojito's run on the lowered CFG, which is why a
slot whose reads merge two stores (R927) keeps its slot, and why every
later pass sees blocks instead of the program's shape. This is the
shape-closing step; it moves Mojito's structure toward upstream more than it
reduces instance count on its own.

One command runs the whole chain in work order:

```sh
scripts/claude_loop.py --task R927 -n 0
```

which is, in order:

| step | entry | what it moves onto regions |
|---|---|---|
| 1 | R875 | the move and initialization checks (`CheckLifetimes`) |
| 2 | R876 | the borrow checks |
| 3 | R877 | drop elaboration (`DestructorInsertion` on region exits, not CFG edges) |
| 4 | R878 | field drops, copy elision, and the lifecycle-call namings |
| 5 | R879 | the pre-elaboration pipeline and the package hold structured bodies |
| 6 | R886 (loop results and valued breaks landed), R1258 (loop arguments and valued continues landed), R1259 (elif and `match` region arguments landed), R1260 (`match` and `comptime for` values landed) | region ops carry values across a region boundary (`mojo-shape`) |
| 7 | R880 | the elaborator unrolls `comptime for` by cloning the region |
| 8 | R927 | `Mem2Reg` through region values |

Six of the eight are `Fable, Planned`, so each gets a plan session first.
A session that cannot finish files what remains under new IDs and the next
run picks them up: after the run, `scripts/roadmap.py deps R927` lists what
is still open underneath, and the same command resumes.

## Then: smaller generators

With bodies structured, each generator shrinks by the passes upstream runs
before elaboration. None of these has an open prerequisite today:

```sh
scripts/claude_loop.py --only R928 --only R926 --only R929 --only R930 -n 0
```

- R928 — `SROA`: aggregate slots split per element (`Fable, Planned`).
- R926 — generators CSE'd before elaboration.
- R929 — CSE merges reads of memory.
- R930 — CSE merges values held in storage.

## Then: the rest of the pipeline on regions

R881 (the post-elaboration pipeline), R882 (the VM runs `hlcf` ops), and
R883 (Pliron lowers structured ops) finish the pipeline-order move:

```sh
scripts/claude_loop.py --task R883 -n 0
```

## Measure

Corpus totals from `benchmarks/instantiation.tsv` (refreshed by
`scripts/instantiation-report`; the overnight gate says when it is stale):

| counter | 10-13 | 10-17 |
|---|---|---|
| concrete functions | 42,409 | 24,864 |
| concrete instructions | 892,391 | 402,905 |
| generators | 16,553 | 8,801 |
| generator instances | 26,868 | 13,804 |
| instances per generator | 1.62 | 1.57 |

The program is less than half its 10-13 size, almost all of it from dead
code removal, inlining, and the unused-argument removal that landed 10-17.
Instances per generator barely moved: a generic body is still copied about
as often as before. Refresh the table after each run above and watch that
ratio.

## Landed so far

The four representation gaps are closed (10-16/17): nested `def` calls are
direct (R1037), defaults are filled at call binding (R1077, R1060),
lifecycle operations are synthesized calls (R1041–R1046), and
`inline-parametric` is one substitution over every binder kind
(R1078–R1081). Unused-argument removal (R974, R975, R1032–R1034, R1039)
followed. What those left behind sits in the `pmir` track below the
region chain (`scripts/roadmap.py list --track pmir`): one entry per cause,
not per shape, as the loop prompt now asks.
