#!/usr/bin/env python3
"""Run `docs/roadmap.md` tasks through Claude Code, one fresh session per task.

Each unchecked entry carries a `Model:` bullet, `Opus|Fable, Planned|Not
Planned`. The model word picks the Claude model; `Planned` makes the session
write a plan file at the repository root and then carry it out, while `Not
Planned` takes the entry as-is. Every task is a new `claude -p` process, so no
context carries over from one task to the next.

Each task gets its own commit, made by the loop once the session ends, with the
session's `commit_msg.txt` as the message. The commit takes every tracked change
plus the files the session created, except new files at the repository root
(plan files, prompts, `commit_msg.txt`), which stay untracked. The loop refuses
to start over uncommitted tracked changes, since the first commit would sweep
them in; `--no-commit` leaves everything uncommitted, as before.

A finished task deletes its entry and renumbers the section, so entries are
tracked by title, not by number: `--until 1.5` resolves to the title 1.5 has
when the loop starts and stops once that entry is gone.

Usage:
  scripts/claude_loop.py --list [--section 1]
  scripts/claude_loop.py                      # the first unchecked task
  scripts/claude_loop.py -n 3                 # the next three
  scripts/claude_loop.py --start 1.4 --until 1.7
  scripts/claude_loop.py --section 3 -n 0     # every task in section 3
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


def build_prompt(task: Task, planned: bool) -> str:
    header = (
        f"Do roadmap task {task.id} from docs/roadmap.md: \"{task.title}\".\n\n"
        "The entry as it stands:\n\n"
        f"{task.body}\n\n"
        "Read AGENTS.md first and follow it exactly: the in-session testing "
        "rule (cargo build plus `cargo run -- run` on the touched fixtures; no "
        "suite binaries, corpus filters, scripts/check, or manifest "
        "regeneration), and the documentation duties.\n"
    )
    if planned:
        plan_file = f"{task.slug}-plan.md"
        work = (
            f"\nThis entry is Planned. First investigate and write the plan to "
            f"`{plan_file}` at the repository root, sliced so each slice has a "
            "stop condition. Then execute every slice of that plan, in order, "
            "in this same session. Writing the plan and stopping is a failure.\n"
        )
    else:
        work = "\nThis entry is Not Planned: take it as-is, with no plan file.\n"
    closing = (
        "\nRun the task to completion without checking in between steps; this "
        "session is unattended and nobody will answer a question. When the "
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
    return header + work + closing


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


def planned_for(args, task: Task) -> bool:
    if args.plan is not None:
        return args.plan
    return task.planned is not False


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
                   help="model for an entry with no Model: bullet (such an entry is Planned)")
    plan = p.add_mutually_exclusive_group()
    plan.add_argument("--plan", dest="plan", action="store_true", default=None,
                      help="plan every task, whatever its entry says")
    plan.add_argument("--no-plan", dest="plan", action="store_false",
                      help="take every task as-is")
    p.add_argument("--permission-mode", default="bypassPermissions")
    p.add_argument("--claude", default="claude", help="claude executable")
    p.add_argument("--claude-arg", action="append", default=[],
                   help="extra argument passed to claude (repeatable)")
    p.add_argument("--keep-going", action="store_true",
                   help="continue past a failed session or an entry left in place "
                        "(committing whatever it left, so the next task's commit stays its own)")
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

    commit = not (args.no_commit or args.dry_run)
    if commit and (dirty := tracked_changes()):
        sys.exit("claude_loop: uncommitted tracked changes would be swept into the first "
                 f"task's commit; commit or stash them, or pass --no-commit:\n{dirty}")

    current = find_by_id(tasks, args.start) if args.start else tasks[0]
    until_title = find_by_id(tasks, args.until).title if args.until else None
    if until_title and tasks.index(find_by_title(tasks, until_title)) < tasks.index(current):
        sys.exit(f"claude_loop: --until {args.until} comes before the start task {current.id}")

    done = 0
    while current is not None:
        idx = tasks.index(current)
        successor = tasks[idx + 1].title if idx + 1 < len(tasks) else None
        planned = planned_for(args, current)
        prompt = build_prompt(current, planned)
        model = model_for(args, current)

        if args.dry_run:
            print(f"== {current.describe()}\n== model {model}\n{prompt}")
            ok, gone = True, True
        else:
            untracked_before = untracked_files() if commit else set()
            msg_before = read_commit_msg()
            label = f"{args.model or current.model or args.default_model}, "
            label += "Planned" if planned else "Not Planned"
            ok = run_claude(args, current, model, label, prompt)
            gone = find_by_title(open_tasks(args.section), current.title) is None
            if commit and ((ok and gone) or args.keep_going):
                commit_task(current, untracked_before, msg_before, ok and gone)
            if not ok:
                print(f"claude_loop: session for \"{current.title}\" failed", file=sys.stderr)
            elif not gone:
                print(f"claude_loop: entry \"{current.title}\" is still on the roadmap",
                      file=sys.stderr)
            if not (ok and gone) and not args.keep_going:
                return 1

        done += 1
        if current.title == until_title or (args.count and done >= args.count):
            break

        if not args.dry_run:
            tasks = open_tasks(args.section)
        current = next_task(tasks, current, successor)
        if until_title and current is not None and find_by_title(tasks, until_title) is None:
            break

    return 0


if __name__ == "__main__":
    sys.exit(main())
