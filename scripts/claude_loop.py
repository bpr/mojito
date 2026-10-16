#!/usr/bin/env python3
"""Run `docs/roadmap.md` tasks through Claude Code, one fresh session per task.

The loop carries each task out: a session takes the entry as it stands, does
the work, deletes the entry, and the loop commits what it left. Every task is a
new `claude -p` process, so no context carries over from one task to the next.
`--plan` asks for a plan file instead, leaving the entry and the code alone.

The loop never stops on a bad task. A session that fails, or that leaves its
entry on the roadmap, is reported and committed as it stands, and the next task
starts; the exit status is 1 if any task went that way, and an entry that waits
on one of those is skipped for the rest of the run. A session a usage limit
cuts off is not a bad task: the loop waits for the limit to reset and resumes
that session with its context, and if the reset is more than six hours away it
stops, leaving the session's work uncommitted and printing how to resume it.
The other thing that does
stop a run is three sessions in a row failing inside a minute, which means the
environment is refusing to work (expired credentials, an exhausted rate limit)
rather than the tasks being hard. Uncommitted tracked
changes present when the loop starts are committed on their own first, so no
task's commit sweeps them up, and `--no-commit` leaves everything uncommitted.

Each task's commit takes every tracked change plus the files the session
created, except new files at the repository root (plan files, prompts,
`commit_msg.txt`), which stay untracked. The message is the session's
`commit_msg.txt`.

Entries are named by their stable IDs (`R12`), which never change, and the
order is `scripts/roadmap.py`'s work order, recomputed before every task: the
open entries top-down, each preceded by its open prerequisites from any track.
`--track` and `--task` narrow the entries the run is for; their prerequisites
still run first. `--only` runs exactly the named entries. `--start` and
`--until` cut a range out of the work order as it stands at launch, and the run
keeps to that range: an entry filed while it runs waits for a later run, even
one that sorts inside the range.

Each entry's `Model:` bullet, `Opus|Fable, Planned|Not Planned`, picks the
Claude model and whether the task is planned first. A `Planned` entry with no
plan file at the repository root gets a plan session before the session that
carries it out, which is handed the plan; a plan pass that fails or writes no
plan file leaves the task unexecuted. `--no-auto-plan` skips that pass,
`--auto-plan-all` gives it to `Not Planned` entries too, `--plan-fable-on-opus`
gives it only to a `Fable` entry that runs on Opus, and a plan file
already at the root is handed to the executing session either way. A plan
counts only while its name matches the entry's current title: one left under
the same ID by an entry since rewritten in place is stale, so the entry counts
as unplanned, and its plan pass reads the old plan as background.
`--model Opus` or `--model Fable` overrides every entry's model choice (say,
when Fable usage runs out); planning still follows the bullet.

Usage:
  scripts/claude_loop.py --list [--track pmir]
  scripts/claude_loop.py                      # carry out the next task
  scripts/claude_loop.py --track pmir -n 0    # everything pmir needs, in order
  scripts/claude_loop.py --task R8 -n 0       # R8 and its open prerequisites
  scripts/claude_loop.py --only R40 --only R41
  scripts/claude_loop.py --start R3 --until R9
  scripts/claude_loop.py --plan -n 3          # plan the next three instead
  scripts/claude_loop.py --no-auto-plan       # execute Planned entries unplanned
  scripts/claude_loop.py --auto-plan-all      # plan every entry, then execute it
  scripts/claude_loop.py --model opus --plan-fable-on-opus  # plan Fable entries Opus runs
  scripts/claude_loop.py --model opus -n 0    # run every entry on Opus
  scripts/claude_loop.py --dry-run -n 2       # print prompts, run nothing
  scripts/claude_loop.py --no-commit          # leave the work uncommitted
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import shutil
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROADMAP = ROOT / "docs" / "roadmap.md"
LOG_DIR = ROOT / "target" / "claude-loop"
COMMIT_MSG = ROOT / "commit_msg.txt"
TICK = 5
QUICK_FAIL = 60
QUICK_FAIL_LIMIT = 3
LIMIT_MAX_WAIT = 6 * 3600
LIMIT_SLACK = 90
RESUME_PROMPT = (
    "Your session was cut off by a usage limit, which has now reset. Continue "
    "the task from where you stopped, under the same instructions, and finish "
    "it.\n"
)

MODELS = {
    "Opus": "claude-opus-5-5[1m]",
    "Fable": "claude-fable-5-1",
}
MODEL_ALIASES = {name.lower(): name for name in MODELS}

sys.path.insert(0, str(Path(__file__).resolve().parent))
import roadmap  # noqa: E402

Task = roadmap.Entry


def pending(args, done: set[str], troubled: set[str],
            fixed: set[str] | None = None) -> list[Task]:
    """The run's remaining tasks in work order, read afresh from the roadmap.

    Keeps to `fixed`, the `--start`/`--until` range taken at launch, when there
    is one, and leaves out what this run already took on and every entry that
    waits, directly or not, on a task that left work behind.
    """
    rm = roadmap.parse()
    if args.only:
        order = [e for e in rm.open() if e.id in args.only]
    elif args.task:
        order = rm.work_order([e for e in rm.open() if e.id in args.task])
    else:
        order = rm.work_order(rm.track_roots(args.track))
    blocked = set(troubled)
    for t in rm.work_order():
        if any(d.id in blocked for d in rm.prerequisites(t)):
            blocked.add(t.id)
    return [t for t in order
            if t.id not in done | blocked and (fixed is None or t.id in fixed)]


def on_roadmap(task: Task) -> bool:
    return any(e.id == task.id and not e.checked for e in roadmap.parse().entries)


def build_prompt(task: Task, mode: str, has_plan: bool | None = None) -> str:
    """The session prompt; `has_plan` overrides looking for the plan file, so a
    dry run's execute prompt after a plan pass reads as the real one will."""
    header = (
        f"Roadmap task {task.id} from docs/roadmap.md (track `{task.track}`): "
        f"\"{task.title}\".\n\n"
        "The entry as it stands:\n\n"
        f"{task.body}\n\n"
        "Read AGENTS.md first and follow it exactly: the in-session testing "
        "rule (cargo build plus `cargo run -- run` on the touched fixtures; no "
        "suite binaries, corpus filters, scripts/check, or manifest "
        "regeneration), and the documentation duties.\n"
        "\nEvery choice between following Mojo and diverging from it is "
        "already decided: follow Mojo, in semantics (match the pin) and in "
        "structure (resemble Mojo's own implementation), even when that is "
        "more work.\n"
        "\nThis is a one-shot unattended session: it ends when your turn ends, "
        "and nothing resumes it. Wait out every background command you start "
        "instead of ending the turn while one is still running, and never "
        "finish on an intention — \"I'll pick this up when it finishes\" picks "
        "up nothing.\n"
    )
    if mode == "plan":
        stale = stale_plans(task)
        background = (
            "\nAn older plan under this ID, "
            + ", ".join(f"`{p.name}`" for p in stale)
            + ", was written before the entry was rewritten in place; part of "
            "it has likely landed. Read it as background only, check against "
            "the code what is still open, and leave it alone.\n"
            if stale else ""
        )
        return header + background + (
            "\nPlan this task; do not carry it out. Investigate the code the "
            f"entry names, then write the plan to `{plan_path(task).name}` at "
            "the repository root, sliced so each slice has a stop condition and "
            "names the files and symbols it touches. Size the task so one "
            "execute session can land all of it; if it needs more, say so in "
            "the plan and name the slices that should become their own "
            "roadmap entries. State what you verified "
            "against the code and what the entry gets wrong, if anything. "
            "The plan must move Mojito closer to Mojo; where an option would "
            "diverge, plan the Mojo-shaped one, and file any unavoidable gap "
            "as a roadmap divergence rather than adopting it.\n"
            "\nNobody will answer a question, so do not check in between "
            "steps. Reading the tree, and building or running "
            "a probe to test a premise, is fine, but the plan file must be the "
            "only change you leave behind: revert any edit you made while "
            "probing, keep probe sources outside the repository, leave the "
            "roadmap entry in place, and touch neither docs/features.md, "
            "CHANGELOG.md, nor commit_msg.txt. Do not commit. If the task turns "
            "out not to be doable, say so in the plan file and explain why. "
            "The plan file is the only thing that makes this task count as "
            "done, so write it before you finish whatever else is unresolved: "
            "if a probe never came back, plan from what you already know and "
            "say what stayed unverified.\n"
        )
    plan = plan_path(task)
    opening = (
        (
            f"\nA plan for this task is already at `{plan.name}`, from an "
            "earlier session. Read it first and follow it where it still holds "
            "against the code; where it does not, do the right thing and say "
            "so. Where the plan picked a Mojito-only option, follow Mojo "
            "instead. Leave the plan file itself alone.\n"
        )
        if (plan.exists() if has_plan is None else has_plan)
        else "\nCarry this task out as-is; there is no plan file for it.\n"
    )
    return header + opening + (
        "\nRun the task to completion without checking in between steps; "
        "nobody will answer a question. When the "
        f"work lands, delete the {task.id} entry from docs/roadmap.md: IDs "
        "are stable, nothing is renumbered, and a Depends bullet naming a "
        "landed ID needs no edit. If the change also fixes another open "
        "entry, confirm it with that entry's probe and delete that entry "
        "too. Record the outcome in docs/features.md and CHANGELOG.md.\n"
        "\nResidues: the roadmap must shrink as work lands, so file causes, "
        "not shapes. For each shape the change leaves unhandled, name the "
        "representation gap or pass rule that makes it special. If an open "
        "entry already names that gap (search docs/roadmap.md for the pass, "
        "function, or construct), add the shape to it as a probe bullet "
        "instead of filing a new entry. Otherwise file one entry for the "
        "gap, stating it in Mojo's terms and listing every shape it causes "
        "as a probe, never one entry per shape. A new entry gets its ID from "
        "`scripts/roadmap.py new-id`, sits in the track that owns its fix at "
        "the position its importance earns, carries a Depends bullet naming "
        "IDs and a Model bullet, and is sized so one session can land it "
        "and delete it. Then run `scripts/roadmap.py lint`.\n\nOverwrite "
        "commit_msg.txt with one short paragraph. If the task turns out not "
        "to be doable, "
        "leave the code honest and say so. If only part of it lands, delete "
        f"the {task.id} entry anyway and file what remains as new entries, "
        "each one session's work, chained by Depends bullets: never rewrite "
        "it as one larger entry that no session can close. If none of it can "
        "land, rewrite the entry to state why. Before finishing: cargo fmt --all, git diff --check, "
        "a clean cargo build, and a clean `cargo clippy --workspace --exclude "
        "mojito-pliron --lib -- -D warnings`. Do not commit: the loop commits "
        "this task's work on its own, with commit_msg.txt as the message.\n"
    )


def plan_path(task: Task) -> Path:
    """Where the task's plan is: `R12-<slug>-plan.md` named after the entry's
    current title, or a plan named by the title alone, from before IDs."""
    legacy = ROOT / f"{task.slug}-plan.md"
    return legacy if legacy.exists() else ROOT / f"{task.id}-{task.slug}-plan.md"


def stale_plans(task: Task) -> list[Path]:
    """Plans under the task's ID written for an earlier title: the entry was
    rewritten in place since, so they plan work that may already have landed."""
    current = plan_path(task)
    return [p for p in sorted(ROOT.glob(f"{task.id}-*-plan.md")) if p != current]


def plan_written(task: Task, since: float) -> bool:
    """Whether the session left a plan file, rather than an older run's."""
    try:
        return plan_path(task).stat().st_mtime >= since - 1
    except OSError:
        return False


@dataclass
class Outcome:
    ok: bool
    session_id: str | None
    limit_reset: float | None
    """When a usage limit cut the session off, the epoch second it resets."""


def run_claude(args, task: Task, model: str, label: str, prompt: str,
               resume: str | None = None) -> Outcome:
    """Run one session, showing only the task and a ticking elapsed-seconds count.

    The session's stream goes to the log alone; the task's line is rewritten in
    place every `TICK` seconds and left showing the total when the session ends.
    `resume` continues an earlier session, its context intact, with `prompt`.
    """
    LOG_DIR.mkdir(parents=True, exist_ok=True)
    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    log_path = LOG_DIR / f"{stamp}-{task.id}-{task.slug}.jsonl"
    cmd = [
        args.claude,
        "-p",
        prompt,
        "--model",
        model,
        "--permission-mode",
        args.permission_mode,
        "--output-format",
        "stream-json",
        "--verbose",
        *(["--resume", resume] if resume else []),
        *args.claude_arg,
    ]
    result: dict = {}
    rejected_until: list[float] = []

    def drain(stdout) -> None:
        with log_path.open("w") as log:
            for line in stdout:
                log.write(line)
                log.flush()
                try:
                    event = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if event.get("type") == "result":
                    result.update(event)
                info = event.get("rate_limit_info") or {}
                if info.get("status") == "rejected" and info.get("resetsAt"):
                    rejected_until.append(float(info["resetsAt"]))

    began = time.monotonic()
    with log_path.with_suffix(".stderr").open("w") as err, subprocess.Popen(
        cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=err, stdin=subprocess.DEVNULL, text=True
    ) as proc:
        reader = threading.Thread(target=drain, args=(proc.stdout,), daemon=True)
        reader.start()
        while True:
            show_progress(task, label, time.monotonic() - began)
            try:
                code = proc.wait(timeout=TICK)
                break
            except subprocess.TimeoutExpired:
                pass
        reader.join()
    ok = code == 0 and not result.get("is_error", False)
    limited = not ok and (result.get("api_error") == "usage_limit_reached"
                          or result.get("api_error_status") == 429)
    reset = (max(rejected_until) if rejected_until else time.time() + 3600) if limited else None
    suffix = "" if ok else "  USAGE LIMIT" if limited else "  FAILED"
    show_progress(task, label, time.monotonic() - began, suffix)
    print(flush=True)
    return Outcome(ok, result.get("session_id"), reset)


def resume_through_limits(args, task: Task, model: str, label: str,
                          outcome: Outcome, attempts: int = 3) -> Outcome:
    """Wait out a usage limit that cut `outcome`'s session off, then resume it.

    The session keeps its context, so the task carries on rather than starting
    over on a tree holding half its work. A limit that resets more than
    `LIMIT_MAX_WAIT` away, or one still in force after `attempts` resumptions,
    is returned as it stands for the caller to stop on.
    """
    for _ in range(attempts):
        if outcome.limit_reset is None or not outcome.session_id:
            return outcome
        wait = outcome.limit_reset + LIMIT_SLACK - time.time()
        if wait > LIMIT_MAX_WAIT:
            return outcome
        until = dt.datetime.fromtimestamp(time.time() + max(wait, 0)).strftime("%H:%M")
        print(f"claude_loop: usage limit; resuming {task.id}'s session at {until}",
              file=sys.stderr, flush=True)
        time.sleep(max(wait, 0))
        outcome = run_claude(args, task, model, f"{label} (resumed)", RESUME_PROMPT,
                             resume=outcome.session_id)
    return outcome


def show_progress(task: Task, label: str, seconds: float, suffix: str = "") -> None:
    line = f"[{seconds:6.0f}s] {label}  {task.title}"
    width = shutil.get_terminal_size().columns - 1 - len(suffix)
    if len(line) > width:
        line = line[: max(width - 3, 0)] + "..."
    print(f"\r\033[K{line}{suffix}", end="", flush=True)


def git(*argv: str, stdin: str | None = None) -> str:
    return subprocess.run(
        ["git", *argv], cwd=ROOT, input=stdin, text=True, capture_output=True, check=True
    ).stdout


def untracked_files() -> set[str]:
    return set(git("ls-files", "--others", "--exclude-standard", "-z").split("\0")) - {""}


def tracked_changes() -> str:
    return git("status", "--porcelain", "--untracked-files=no")


def read_commit_msg() -> str:
    try:
        return COMMIT_MSG.read_text().strip()
    except OSError:
        return ""


def commit_stray() -> None:
    """Commit the tracked changes already in the tree, before the first task.

    Nothing is discarded and no untracked file is touched; the changes simply
    stop being something a task's own commit would sweep up.
    """
    git("add", "--update")
    if git("diff", "--cached", "--name-only"):
        git("commit", "--quiet", "--file", "-",
            stdin="Work already in the tree when the loop started\n\n"
                  "Committed by scripts/claude_loop.py so the first task's commit "
                  "stays its own.\n")


def commit_task(task: Task, untracked_before: set[str], msg_before: str, finished: bool) -> None:
    """Commit the session's work for `task` as one commit.

    Stages every tracked change and each file the session created below the
    repository root; new root-level files are scratch and stay untracked.
    """
    created = sorted(untracked_files() - untracked_before)
    staged = [f for f in created if "/" in f]
    git("add", "--update")
    if staged:
        git("add", "--", *staged)
    if not git("diff", "--cached", "--name-only"):
        return
    msg = read_commit_msg()
    if not msg or msg == msg_before:
        msg = f"Roadmap {task.id}: {task.title}"
        if not finished:
            msg += "\n\nThe session ended without removing the roadmap entry."
    git("commit", "--quiet", "--file", "-", stdin=msg + "\n")


def model_name(args, task: Task) -> str:
    """The model the task runs on: `--model` when given, else the entry's
    `Model:` bullet, else `--default-model`."""
    if args.model:
        return MODEL_ALIASES.get(args.model.lower(), args.model)
    return task.model or args.default_model


def model_for(args, task: Task) -> str:
    name = model_name(args, task)
    return {"Opus": args.opus_model, "Fable": args.fable_model}.get(name, name)


def modes_for(args, task: Task) -> list[str]:
    """The task's sessions in order: `"plan"` writes the plan file only,
    `"execute"` carries the task out. A `Planned` entry (any entry, under
    `--auto-plan-all`, and a `Fable` entry run on Opus, under
    `--plan-fable-on-opus`) with no plan file yet is planned before it is
    carried out."""
    if args.plan:
        return ["plan"]
    downgraded = task.model == "Fable" and model_name(args, task) == "Opus"
    wants_plan = (task.planned or args.auto_plan_all
                  or (args.plan_fable_on_opus and downgraded))
    if wants_plan and not args.no_auto_plan and not plan_path(task).exists():
        return ["plan", "execute"]
    return ["execute"]


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("-n", "--count", type=int,
                   help="tasks to run; 0 means no limit (default 1, or no limit with --until or --only)")
    p.add_argument("--track", help="only the entries of this track, and what they need")
    p.add_argument("--task", action="append", metavar="ID",
                   help="only this entry and its open prerequisites (repeatable)")
    p.add_argument("--only", action="append", metavar="ID",
                   help="exactly this entry, prerequisites or not (repeatable)")
    p.add_argument("--start", metavar="ID", help="begin the run at this entry of the launch-time work order")
    p.add_argument("--until", metavar="ID", help="stop after this entry")
    p.add_argument("--list", action="store_true", help="list the work order and exit")
    p.add_argument("--dry-run", action="store_true", help="print each prompt instead of running it")
    p.add_argument("--model", metavar="Opus|Fable|ID",
                   help="use this model for every task, ignoring Model: bullets: "
                        "Opus or Fable (any case, mapped through --opus-model/--fable-model) "
                        "or a literal claude model ID")
    p.add_argument("--opus-model", default=MODELS["Opus"])
    p.add_argument("--fable-model", default=MODELS["Fable"])
    p.add_argument("--default-model", choices=MODELS, default="Opus",
                   help="model for an entry with no Model: bullet")
    p.add_argument("--plan", action="store_true",
                   help="write each task's plan file instead of carrying the task out")
    auto_plan = p.add_mutually_exclusive_group()
    auto_plan.add_argument("--no-auto-plan", action="store_true",
                           help="carry Planned entries out without a plan session first")
    auto_plan.add_argument("--auto-plan-all", action="store_true",
                           help="give every entry without a plan file a plan session first, "
                                "Not Planned ones included")
    auto_plan.add_argument("--plan-fable-on-opus", action="store_true",
                           help="also give a plan session to a Fable entry that runs on Opus "
                                "(through --model), Not Planned or not")
    p.add_argument("--permission-mode", default="bypassPermissions")
    p.add_argument("--claude", default="claude", help="claude executable")
    p.add_argument("--claude-arg", action="append", default=[],
                   help="extra argument passed to claude (repeatable)")
    p.add_argument("--no-commit", action="store_true",
                   help="do not commit each finished task")
    args = p.parse_args()
    if args.count is None:
        args.count = 0 if args.until or args.only else 1
    if sum(map(bool, (args.track, args.task, args.only))) > 1:
        sys.exit("claude_loop: --track, --task, and --only exclude each other")
    rm = roadmap.parse()
    args.task = [rm.find(i).id for i in args.task or []]
    args.only = [rm.find(i).id for i in args.only or []]
    args.start, args.until = (rm.find(i).id if i else None for i in (args.start, args.until))

    done: set[str] = set()
    troubled: set[str] = set()
    tasks = pending(args, done, troubled)
    if args.list:
        for t in tasks:
            print(t.describe())
        return 0
    ids = [t.id for t in tasks]
    for flag, ident in (("--start", args.start), ("--until", args.until)):
        if ident and ident not in ids:
            sys.exit(f"claude_loop: {flag} {ident} is not in this run's work order")
    until = args.until
    fixed = None
    if args.start or until:
        first = ids.index(args.start) if args.start else 0
        last = ids.index(until) if until else len(ids) - 1
        if last < first:
            sys.exit(f"claude_loop: --until {until} comes before the start task {args.start}")
        fixed = set(ids[first:last + 1])
    if not tasks:
        print("claude_loop: no open tasks")
        return 0

    commit = not (args.no_commit or args.dry_run) and not args.plan
    if commit and (dirty := tracked_changes()):
        print("claude_loop: committing the changes already in the tree on their own, so "
              f"the first task's commit stays its own:\n{dirty}", file=sys.stderr)
        commit_stray()

    ran = 0
    quick = 0
    stopped_by_limit: tuple[Task, Outcome] | None = None
    while tasks := pending(args, done, troubled, fixed):
        current = tasks[0]
        modes = modes_for(args, current)
        for stale in stale_plans(current):
            print(f"claude_loop: {stale.name} was written for an earlier title of "
                  f"{current.id}; not handing it to the session as its plan",
                  file=sys.stderr)
        for mode in modes:
            prompt = build_prompt(current, mode, True if "plan" in modes else None)
            model = model_for(args, current)

            if args.dry_run:
                print(f"== {current.describe()}\n== model {model} ({mode})\n{prompt}")
                continue
            committing = commit and mode == "execute"
            untracked_before = untracked_files() if committing else set()
            msg_before = read_commit_msg()
            label = f"{current.id} {model_name(args, current)}, {mode}"
            began = time.time()
            outcome = resume_through_limits(
                args, current, model, label, run_claude(args, current, model, label, prompt))
            if outcome.limit_reset is not None:
                stopped_by_limit = (current, outcome)
                break
            ok = outcome.ok
            quick = quick + 1 if not ok and time.time() - began < QUICK_FAIL else 0
            if mode == "plan":
                landed = plan_written(current, began)
                if ok and not landed:
                    print(f"claude_loop: session for {current.id} wrote no "
                          f"{plan_path(current).name}", file=sys.stderr)
            else:
                landed = not on_roadmap(current)
                if committing:
                    commit_task(current, untracked_before, msg_before, ok and landed)
                if ok and not landed:
                    print(f"claude_loop: entry {current.id} is still on the roadmap",
                          file=sys.stderr)
            if not ok:
                print(f"claude_loop: session for {current.id} failed", file=sys.stderr)
            if not (ok and landed):
                troubled.add(current.id)
                break

        if stopped_by_limit:
            break
        done.add(current.id)
        ran += 1
        if quick >= QUICK_FAIL_LIMIT:
            print(f"claude_loop: {quick} sessions in a row failed inside "
                  f"{QUICK_FAIL}s, which is the environment rather than the "
                  "tasks; stopping with the rest of the run untouched",
                  file=sys.stderr)
            break
        if current.id == until or (args.count and ran >= args.count):
            break
        if until and not args.dry_run and not any(t.id == until for t in roadmap.parse().open()):
            break

    if stopped_by_limit:
        task, outcome = stopped_by_limit
        reset = dt.datetime.fromtimestamp(outcome.limit_reset).strftime("%Y-%m-%d %H:%M")
        print(f"claude_loop: a usage limit stopped {task.id}'s session until {reset}; "
              "its work is left uncommitted in the tree. Continue that session with "
              f"`claude --resume {outcome.session_id}`, or rerun the loop, which "
              "commits the tree on its own first and starts the task afresh.",
              file=sys.stderr)
        return 1
    if troubled:
        print(f"claude_loop: {ran} tasks ran, {len(troubled)} left work behind:",
              file=sys.stderr)
        for ident in sorted(troubled, key=lambda i: int(i[1:])):
            print(f"  {ident}", file=sys.stderr)
    return 1 if troubled else 0


if __name__ == "__main__":
    sys.exit(main())
