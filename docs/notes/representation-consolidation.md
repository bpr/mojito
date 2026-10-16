# Shrinking Generic Code Early: Fix Representations, Not Shapes

Written 2026-10-16, after R923 landed, in answer to: *how do we move Mojito
closer to Mojo in as few steps as possible, with the goal of downsizing the
program representation as early as possible to prevent code explosion during
generic instantiation, and why does looping over `pmir` keep growing the
list?*

## Why the `pmir` list kept growing

Each task was scoped to one shape ("a `nodebug` callee with X is not
inlined"), and the loop prompt told the session to file every shape it did
not handle as its own one-session entry. So each landing added two to five
entries (R923 filed three, R1014 five), and the track grew as work landed.

Most of those entries were not independent defects. They were symptoms of
four places where Mojito encodes one concept several ways and upstream
encodes it once. A pass written against the irregular encoding needs one
case per shape. Make the encoding uniform and the pass needs no cases, so
the shapes stop being entries at all.

## The four representation gaps

| Gap | Upstream shape | Entry now | Folded in |
|---|---|---|---|
| A call of a nested `def` is `closure.make` + `call.indirect`, and passes recover the direct call by dataflow | Direct call of the lifted generator, captures passed as arguments | R1037 | R1036 |
| Evaluated defaults are filled late, in MIR, per call shape (`mir::evaluate_omitted_defaults`), and every other shape is left to the callee | Call binding writes every omitted argument into the call | landed 2026-10-16 (R1077, R1060; residues R1083, R1084) | R1053–R1059, R1061–R1063, R960, R961 |
| Drops and copies are their own instructions, and the backends recurse into fields by type | A synthesized `__del__`/copy constructor, called by a plain call | R1041, R1044, R1045, R1042, R1043, R1046 (existing chain) | — |
| `inline-parametric` substitutes some binder kinds and refuses the rest, one entry per refusal | One substitution over every parameter kind | R1078, R1079, R1080, R1081 | R945, R947–R949, R951, R952, R954, R955, R957–R959, R962–R968, R1067, R1068 |

The first three are prerequisites of the fourth. With nested calls direct,
defaults explicit at the call, and drops and copies as calls, most of the
inliner's refusals disappear rather than needing code. The `pmir` track went
from 75 open entries to 47 with this consolidation.

Why this serves the downsizing goal: every one of these makes the
pre-elaboration passes (`inline-parametric`, unused-argument removal, dead
symbol elimination) apply to more of the program before `native::mono`
instantiates it, so each instance is smaller and fewer instances are keyed
by binders nothing reads.

## Order of work

1. **R1037, nested `def` calls are direct.** No dependencies. It retires
   the `escaping_nested_bodies` dataflow (`callable_targets` stays for
   callable binders and function-value calls) and unblocks R1080 and
   R1038.
2. **R1077, then R1060, defaults filled at call binding.** Landed
   2026-10-16: the lowering of every checked call records its callee, and
   `mir::fill_omitted_arguments` writes each omitted argument into the
   call once every declaration is lowered. The callee-side evaluation and
   `evaluated_at_call_site` are gone.
3. **R1041 → R1046, lifecycle operations as calls.** Synthesized
   destructors and copy/move constructors, cleanup lists naming
   destructors, then drops and copies lowered to calls.
   - Risk: R1043 says Pliron's drop flags and the VM's partial-aggregate
     rule exist because drop states are not definite at every death. If its
     session finds that needs the structured-region lifetime checks
     (`mojo-shape` R875–R877) first, it should file that dependency, not
     force the change.
4. **R1078 → R1081, one inliner.** Generic binder substitution first, then
   collectors and spreads, nested `def`s, and destructors. All four build on
   R1078's single binding map.

After these, the structured-region work on `mojo-shape` (R875–R879) is the
next shape-closing step. It moves Mojito's structure toward upstream more
than it reduces instance count.

## The loop prompt change

`scripts/claude_loop.py` now tells each session to file *causes, not
shapes*:

- Before filing a residue, name the gap that makes the shape special.
- If an open entry already names it, add the shape there as a probe bullet.
- Otherwise file one entry for the gap, listing every shape as a probe.
- If the change also fixes another open entry, confirm it by that entry's
  probe and delete it too.

## Commands

The work order is computed, and these IDs are contiguous at the top of the
`pmir` track, so one range covers them all:

```sh
scripts/claude_loop.py --start R1037 --until R1081 -n 0
```

Or one goal at a time, checking the roadmap between runs:

```sh
# 1. nested defs are direct calls
scripts/claude_loop.py --only R1037 -n 0
# 2. defaults filled at call binding
scripts/claude_loop.py --start R1077 --until R1060 -n 0
# 3. lifecycle operations as calls
scripts/claude_loop.py --start R1041 --until R1046 -n 0
# 4. one inliner
scripts/claude_loop.py --start R1078 --until R1081 -n 0
```

Most of these entries are `Fable, Planned`, so each gets a plan session
before its execution session. Add `--model opus --plan-fable-on-opus` if
Fable usage runs short. Preview any run with `--dry-run`, or list the order
with `scripts/roadmap.py list --track pmir`.

A range is fixed at launch: entries filed during the run wait for a later
run, even when they sort inside the range. After the run, check
`scripts/roadmap.py list --track pmir | head` for anything new at the top.
