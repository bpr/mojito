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
| `inline-parametric` substitutes some binder kinds and refuses the rest, one entry per refusal | One substitution over every parameter kind | landed 2026-10-17 (R1078, R1079, R1080, R1081; residues R1096–R1106) | R945, R947–R949, R951, R952, R954, R955, R957–R959, R962–R968, R1067, R1068 |

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

## Resuming after the 2026-10-16 usage-limit stop

The first run landed R1037, R1077 with R1060, R1041, R1044, R1045, and
R1042. The account's usage limit then cut off R1043's session 95 turns
in. The loop committed that session's unfinished work as 9867474f, which
builds and passes Clippy but is unverified: the session was still
debugging one Pliron/VM difference and six drops the new `drop_calls`
pass had not lowered. It had also reserved R1087 and R1088 without
filing them. R1078's plan session hit the same limit before doing
anything.

The loop now waits for a usage limit to reset and resumes the cut-off
session, so this will not happen again unattended.

Run these from the repository root.

1. Resume R1043's session, which still holds its context:

   ```sh
   claude --resume 310c58a3-0254-49a1-8c6e-f889d2d4da1f
   ```

   Tell it: "You were cut off by a usage limit. Continue R1043 from where
   you stopped and finish it, including deleting the roadmap entry and
   overwriting commit_msg.txt."

2. When the session is done and R1043 is gone from `docs/roadmap.md`,
   commit its work under its own message:

   ```sh
   git add -A crates tests docs assets && git commit -F commit_msg.txt
   ```

   If you skip this, the next loop run commits the tree on its own with a
   generic message, and that commit also takes the loop fix itself.

3. Finish the rest of the sequence. `--track pmir` is used rather than a
   `--start` range, so the new residues that now sort first (R1083, R1085)
   run too:

   ```sh
   scripts/claude_loop.py --track pmir --until R1081 -n 0
   ```

Alternative to steps 1–2: run step 3 alone. The loop then commits the tree
first and a fresh R1043 session starts over from the partial code in HEAD,
without the earlier session's context.
