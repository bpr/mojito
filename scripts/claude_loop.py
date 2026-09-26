#!/usr/bin/env python3
"""Run `docs/roadmap.md` tasks through Claude Code, one fresh session per task.

The loop carries each task out: a session takes the entry as it stands, does
the work, deletes the entry, and the loop commits what it left. Every task is a
new `claude -p` process, so no context carries over from one task to the next.
`--plan` asks for a plan file instead, leaving the entry and the code alone.

The loop never stops on a bad task. A session that fails, or that leaves its
entry on the roadmap, is reported and committed as it stands, and the next task
starts; the exit status is 1 if any task went that way. The one thing that does
stop a run is three sessions in a row failing inside a minute, which means the
environment is refusing to work (expired credentials, an exhausted rate limit)
rather than the tasks being hard. Uncommitted tracked
changes present when the loop starts are committed on their own first, so no
task's commit sweeps them up, and `--no-commit` leaves everything uncommitted.

Each task's commit takes every tracked change plus the files the session
created, except new files at the repository root (plan files, prompts,
`commit_msg.txt`), which stay untracked. The message is the session's
`commit_msg.txt`.

A carried-out task deletes its entry and renumbers the section, so entries are
tracked by title, not by number: `--until 1.5` resolves to the title 1.5 has
when the loop starts and stops once that entry is gone.

Each entry's `Model:` bullet, `Opus|Fable, Planned|Not Planned`, picks the
Claude model; the `Planned` word only says whether the entry wanted a plan
first, and a plan file already at the repository root is handed to the session
that carries its task out.

Usage:
  scripts/claude_loop.py --list [--section 1]
  scripts/claude_loop.py                      # carry out the first unchecked task
  scripts/claude_loop.py --start 1.1 --until 1.27
  scripts/claude_loop.py --section 3 -n 0     # every task in section 3
  scripts/claude_loop.py --plan -n 3          # plan the next three instead
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

MODELS = {
    "Opus": "claude-opus-5-5[1m]",
    "Fable": "claude-fable-5-1",
}

ENTRY_RE = re.compile(r"^- \[(?P<mark>[ xX])\] \*\*(?P<sec>\d+)\.(?P<num>\d+) (?P<rest>.*)$")
BOUNDARY_RE = re.compile(r"^(- \[[ xX]\] |#{1,3} )")
MODEL_RE = re.compile(r"Model:\s*(?P<model>Opus|Fable),\s*(?P<plan>Planned|Not Planned)")


@dataclass
class Task:
    section: int
    number: int
    title: str
    body: str
    checked: bool
    model: str | None
    planned: bool | None

    @property
    def id(self) -> str:
        return f"{self.section}.{self.number}"

    @property
    def slug(self) -> str:
        words = re.sub(r"[^a-z0-9]+", "-", self.title.lower()).strip("-").split("-")
        return "-".join(words[:6])

    def describe(self) -> str:
        model = self.model or "?"
        plan = {True: "Planned", False: "Not Planned", None: "?"}[self.planned]
        return f"{self.id:>6}  {model:<5}  {plan:<11}  {self.title}"


def parse_roadmap(path: Path = ROADMAP) -> list[Task]:
    lines = path.read_text().splitlines()
    tasks: list[Task] = []
    i = 0
    while i < len(lines):
        m = ENTRY_RE.match(lines[i])
        if not m:
            i += 1
            continue
        title_parts = [m["rest"]]
        j = i + 1
        while "**" not in title_parts[-1] and j < len(lines) and lines[j].strip():
            title_parts.append(lines[j].strip())
            j += 1
        title = " ".join(title_parts).split("**", 1)[0].strip()
        k = i + 1
        while k < len(lines) and not BOUNDARY_RE.match(lines[k]):
            k += 1
        body = "\n".join(lines[i:k]).rstrip()
        mm = MODEL_RE.search(body)
        tasks.append(
            Task(
                section=int(m["sec"]),
                number=int(m["num"]),
                title=title,
                body=body,
                checked=m["mark"] != " ",
                model=mm["model"] if mm else None,
                planned=(mm["plan"] == "Planned") if mm else None,
            )
        )
        i = k
    return tasks


def open_tasks(section: int | None) -> list[Task]:
    return [
        t
        for t in parse_roadmap()
        if not t.checked and (section is None or t.section == section)
    ]


def find_by_id(tasks: list[Task], ident: str) -> Task:
    for t in tasks:
        if t.id == ident:
            return t
    sys.exit(f"claude_loop: no unchecked roadmap entry {ident}")


def find_by_title(tasks: list[Task], title: str) -> Task | None:
    return next((t for t in tasks if t.title == title), None)


def next_task(tasks: list[Task], previous: Task, successor: str | None) -> Task | None:
    """The entry that followed `previous` when it started, found by title.

    Falls back to the entry after `previous` when it was left in place, and to
    the first open entry when both are gone.
    """
    if successor and (found := find_by_title(tasks, successor)):
        return found
    if (left := find_by_title(tasks, previous.title)) is not None:
        idx = tasks.index(left) + 1
        return tasks[idx] if idx < len(tasks) else None
    return tasks[0] if tasks else None


def build_prompt(task: Task, mode: str) -> str:
    header = (
        f"Roadmap task {task.id} from docs/roadmap.md: \"{task.title}\".\n\n"
        "The entry as it stands:\n\n"
        f"{task.body}\n\n"
        "Read AGENTS.md first and follow it exactly: the in-session testing "
        "rule (cargo build plus `cargo run -- run` on the touched fixtures; no "
        "suite binaries, corpus filters, scripts/check, or manifest "
        "regeneration), and the documentation duties.\n"
        "\nThis is a one-shot unattended session: it ends when your turn ends, "
        "and nothing resumes it. Wait out every background command you start "
        "instead of ending the turn while one is still running, and never "
        "finish on an intention — \"I'll pick this up when it finishes\" picks "
        "up nothing.\n"
    )
    if mode == "plan":
        return header + (
            "\nPlan this task; do not carry it out. Investigate the code the "
            f"entry names, then write the plan to `{plan_path(task).name}` at "
            "the repository root, sliced so each slice has a stop condition and "
            "names the files and symbols it touches. State what you verified "
            "against the code and what the entry gets wrong, if anything.\n"
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
            "so. Leave the plan file itself alone.\n"
        )
        if plan.exists()
        else "\nCarry this task out as-is; there is no plan file for it.\n"
    )
    return header + opening + (
        "\nRun the task to completion without checking in between steps; "
        "nobody will answer a question. When the "
        "work lands, delete the entry from docs/roadmap.md and renumber the "
        "section (rechecking every number and every \"Depends on\"), record "
        "the outcome in docs/features.md and CHANGELOG.md, file any residue "
        "or divergence as new roadmap entries, and overwrite commit_msg.txt "
        "with one short paragraph. If the task turns out not to be doable, "
        "leave the code honest, rewrite the entry to state what remains and "
        "why, and say so. Before finishing: cargo fmt --all, git diff --check, "
        "a clean cargo build, and a clean `cargo clippy --workspace --exclude "
        "mojito-pliron --lib -- -D warnings`. Do not commit: the loop commits "
        "this task's work on its own, with commit_msg.txt as the message.\n"
    )


def plan_path(task: Task) -> Path:
    return ROOT / f"{task.slug}-plan.md"


def plan_written(task: Task, since: float) -> bool:
    """Whether the session left a plan file, rather than an older run's."""
    try:
        return plan_path(task).stat().st_mtime >= since - 1
    except OSError:
        return False


def run_claude(args, task: Task, model: str, label: str, prompt: str) -> bool:
    """Run one session, showing only the task and a ticking elapsed-seconds count.

    The session's stream goes to the log alone; the task's line is rewritten in
    place every `TICK` seconds and left showing the total when the session ends.
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
        *args.claude_arg,
    ]
    result: dict = {}

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
    show_progress(task, label, time.monotonic() - began, "" if ok else "  FAILED")
    print(flush=True)
    return ok


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
        msg = f"Roadmap: {task.title}"
        if not finished:
            msg += "\n\nThe session ended without removing the roadmap entry."
    git("commit", "--quiet", "--file", "-", stdin=msg + "\n")


def model_for(args, task: Task) -> str:
    if args.model:
        return args.model
    name = task.model or args.default_model
    return {"Opus": args.opus_model, "Fable": args.fable_model}[name]


def mode_for(args, task: Task) -> str:
    """`"plan"` to write the plan file only, `"execute"` to carry the task out."""
    return "plan" if args.plan else "execute"


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("-n", "--count", type=int,
                   help="tasks to run; 0 means no limit (default 1, or no limit with --until)")
    p.add_argument("--start", metavar="N.M", help="first task (default: first unchecked)")
    p.add_argument("--until", metavar="N.M", help="stop after this task (numbered as the loop starts)")
    p.add_argument("--section", type=int, help="only take tasks from this section")
    p.add_argument("--list", action="store_true", help="list unchecked tasks and exit")
    p.add_argument("--dry-run", action="store_true", help="print each prompt instead of running it")
    p.add_argument("--model", help="use this model for every task, ignoring Model: bullets")
    p.add_argument("--opus-model", default=MODELS["Opus"])
    p.add_argument("--fable-model", default=MODELS["Fable"])
    p.add_argument("--default-model", choices=MODELS, default="Opus",
                   help="model for an entry with no Model: bullet")
    p.add_argument("--plan", action="store_true",
                   help="write each task's plan file instead of carrying the task out")
    p.add_argument("--permission-mode", default="bypassPermissions")
    p.add_argument("--claude", default="claude", help="claude executable")
    p.add_argument("--claude-arg", action="append", default=[],
                   help="extra argument passed to claude (repeatable)")
    p.add_argument("--no-commit", action="store_true",
                   help="do not commit each finished task")
    args = p.parse_args()
    if args.count is None:
        args.count = 0 if args.until else 1

    tasks = open_tasks(args.section)
    if args.list:
        for t in tasks:
            print(t.describe())
        return 0
    if not tasks:
        print("claude_loop: no unchecked tasks")
        return 0

    commit = not (args.no_commit or args.dry_run) and not args.plan
    if commit and (dirty := tracked_changes()):
        print("claude_loop: committing the changes already in the tree on their own, so "
              f"the first task's commit stays its own:\n{dirty}", file=sys.stderr)
        commit_stray()

    current = find_by_id(tasks, args.start) if args.start else tasks[0]
    until_title = find_by_id(tasks, args.until).title if args.until else None
    if until_title and tasks.index(find_by_title(tasks, until_title)) < tasks.index(current):
        sys.exit(f"claude_loop: --until {args.until} comes before the start task {current.id}")

    done = 0
    quick = 0
    troubled: list[str] = []
    while current is not None:
        idx = tasks.index(current)
        successor = tasks[idx + 1].title if idx + 1 < len(tasks) else None
        mode = mode_for(args, current)
        prompt = build_prompt(current, mode)
        model = model_for(args, current)

        if args.dry_run:
            print(f"== {current.describe()}\n== model {model} ({mode})\n{prompt}")
            ok, landed = True, True
        else:
            committing = commit and mode == "execute"
            untracked_before = untracked_files() if committing else set()
            msg_before = read_commit_msg()
            label = f"{args.model or current.model or args.default_model}, {mode}"
            began = time.time()
            ok = run_claude(args, current, model, label, prompt)
            quick = quick + 1 if not ok and time.time() - began < QUICK_FAIL else 0
            if mode == "plan":
                landed = plan_written(current, began)
                if ok and not landed:
                    print(f"claude_loop: session for \"{current.title}\" wrote no "
                          f"{plan_path(current).name}", file=sys.stderr)
            else:
                landed = find_by_title(open_tasks(args.section), current.title) is None
                if committing:
                    commit_task(current, untracked_before, msg_before, ok and landed)
                if ok and not landed:
                    print(f"claude_loop: entry \"{current.title}\" is still on the roadmap",
                          file=sys.stderr)
            if not ok:
                print(f"claude_loop: session for \"{current.title}\" failed", file=sys.stderr)
            if not (ok and landed):
                troubled.append(current.title)

        done += 1
        if quick >= QUICK_FAIL_LIMIT:
            print(f"claude_loop: {quick} sessions in a row failed inside "
                  f"{QUICK_FAIL}s, which is the environment rather than the "
                  "tasks; stopping with the rest of the run untouched",
                  file=sys.stderr)
            break
        if current.title == until_title or (args.count and done >= args.count):
            break

        if not args.dry_run:
            tasks = open_tasks(args.section)
        current = next_task(tasks, current, successor)
        if until_title and current is not None and find_by_title(tasks, until_title) is None:
            break

    if troubled:
        print(f"claude_loop: {done} tasks ran, {len(troubled)} left work behind:",
              file=sys.stderr)
        for title in troubled:
            print(f"  {title}", file=sys.stderr)
    return 1 if troubled else 0


if __name__ == "__main__":
    sys.exit(main())
