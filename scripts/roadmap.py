#!/usr/bin/env python3
"""Read `docs/roadmap.md`: stable entry IDs, tracks, dependencies, work order.

Every entry carries a stable ID (`R12`) that is never renumbered or reused,
so landing, filing, or moving an entry edits that entry alone. An entry's
place in its track is its priority. Its `Depends on` bullet names the IDs it
needs, and an ID no longer on the roadmap counts as done. The work order is a
depth-first walk of the open entries in file order that puts each entry's
open prerequisites, from any track, immediately before it.

Commands:
  scripts/roadmap.py list [--track T]   open entries in work order
  scripts/roadmap.py next [--track T]   the entry to do next
  scripts/roadmap.py show R12           one entry as it stands
  scripts/roadmap.py deps R12           its open prerequisites, in work order
  scripts/roadmap.py tracks             each track's slug, title, open count
  scripts/roadmap.py new-id [-n N]      reserve fresh IDs (bumps the header)
  scripts/roadmap.py lint               check IDs, tracks, Depends, Model
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROADMAP = ROOT / "docs" / "roadmap.md"

ENTRY_RE = re.compile(r"^- \[(?P<mark>[ xX])\] \*\*R(?P<num>\d+) (?P<rest>.*)$")
OLD_ENTRY_RE = re.compile(r"^- \[[ xX]\] \*\*\d+\.\d+ ")
TRACK_RE = re.compile(r"^Track: `(?P<slug>[a-z0-9-]+)`")
NEXT_ID_RE = re.compile(r"(?P<pre>Next free ID: \*\*R)(?P<num>\d+)(?P<post>\*\*)")
DEPENDS_RE = re.compile(r"^  - Depends on\b(?P<text>.*(?:\n    .*)*)", re.MULTILINE)
MODEL_RE = re.compile(
    r"^  - Model: (?P<model>Opus|Fable), (?P<plan>Planned|Not Planned)\.?\s*$", re.MULTILINE
)
ID_RE = re.compile(r"\bR(\d+)\b")
ALL_IN_TRACK = "every other entry in this track"


@dataclass
class Entry:
    num: int
    title: str
    body: str
    track: str
    line: int
    checked: bool
    standing: bool
    model: str | None
    planned: bool | None
    depends: list[str] = field(default_factory=list)
    depends_bullets: int = 0
    depends_on_track: bool = False

    @property
    def id(self) -> str:
        return f"R{self.num}"

    @property
    def slug(self) -> str:
        words = re.sub(r"[^a-z0-9]+", "-", self.title.lower()).strip("-").split("-")
        return "-".join(words[:6])

    def describe(self) -> str:
        model = self.model or "?"
        plan = {True: "Planned", False: "Not Planned", None: "?"}[self.planned]
        return f"{self.id:>5}  {self.track:<12}  {model:<5}  {plan:<11}  {self.title}"


@dataclass
class Track:
    slug: str | None
    title: str
    line: int


@dataclass
class Roadmap:
    entries: list[Entry]
    tracks: list[Track]
    next_id: int | None
    errors: list[str]

    def by_id(self) -> dict[str, Entry]:
        return {e.id: e for e in self.entries}

    def open(self) -> list[Entry]:
        return [e for e in self.entries if not e.checked]

    def prerequisites(self, entry: Entry) -> list[Entry]:
        """The open entries `entry` names in its Depends bullet, in file order."""
        index = {e.id: e for e in self.open()}
        if entry.depends_on_track:
            return [
                e
                for e in self.open()
                if e.track == entry.track and e is not entry and not e.depends_on_track
            ]
        return [index[d] for d in entry.depends if d in index and d != entry.id]

    def work_order(self, roots: list[Entry] | None = None) -> list[Entry]:
        """Open non-standing entries, each preceded by its open prerequisites.

        `roots` defaults to every open entry in file order; a track or a task
        list narrows it, and prerequisites still come from anywhere.
        """
        order: list[Entry] = []
        state: dict[str, str] = {}

        def visit(e: Entry) -> None:
            if state.get(e.id):
                return
            state[e.id] = "active"
            for dep in self.prerequisites(e):
                visit(dep)
            state[e.id] = "done"
            order.append(e)

        for root in self.open() if roots is None else roots:
            visit(root)
        return [e for e in order if not e.standing]

    def track_roots(self, track: str | None) -> list[Entry] | None:
        if track is None:
            return None
        if track not in {t.slug for t in self.tracks}:
            sys.exit(f"roadmap: no track `{track}` (see `scripts/roadmap.py tracks`)")
        return [e for e in self.open() if e.track == track]

    def find(self, ident: str) -> Entry:
        ident = ident.upper()
        if not ident.startswith("R"):
            ident = "R" + ident
        found = self.by_id().get(ident)
        if found is None:
            sys.exit(f"roadmap: no entry {ident} on the roadmap")
        return found


def parse(path: Path = ROADMAP) -> Roadmap:
    lines = path.read_text().splitlines()
    entries: list[Entry] = []
    tracks: list[Track] = []
    errors: list[str] = []
    next_id = None
    in_work = False
    track: Track | None = None
    i = 0
    while i < len(lines):
        line = lines[i]
        if next_id is None and (m := NEXT_ID_RE.search(line)):
            next_id = int(m["num"])
        if line.startswith("## "):
            in_work = line.strip() == "## Ordered Work"
            track = None
        elif in_work and line.startswith("### "):
            track = Track(None, line[4:].strip(), i + 1)
            tracks.append(track)
        elif in_work and track and track.slug is None and (m := TRACK_RE.match(line)):
            track.slug = m["slug"]
        elif in_work and OLD_ENTRY_RE.match(line):
            errors.append(f"line {i + 1}: positional entry number; give it an ID")
        elif in_work and (m := ENTRY_RE.match(line)):
            i = parse_entry(lines, i, m, track, entries)
            continue
        i += 1
    return Roadmap(entries, tracks, next_id, errors)


def lint(rm: Roadmap) -> list[str]:
    errors = list(rm.errors)
    if rm.next_id is None:
        errors.append("header: no `Next free ID: **R<n>**` line")
    slugs: dict[str, Track] = {}
    for t in rm.tracks:
        if t.slug is None:
            errors.append(f"line {t.line}: track `{t.title}` has no `Track: \\`slug\\`` line")
        elif t.slug in slugs:
            errors.append(f"line {t.line}: track slug `{t.slug}` is used twice")
        else:
            slugs[t.slug] = t
    seen: dict[str, Entry] = {}
    for e in rm.entries:
        where = f"line {e.line} {e.id}"
        if e.id in seen:
            errors.append(f"{where}: ID already used at line {seen[e.id].line}")
        seen[e.id] = e
        if rm.next_id is not None and e.num >= rm.next_id:
            errors.append(f"{where}: ID is not below the next free ID R{rm.next_id}")
        if e.standing:
            continue
        if e.depends_bullets != 1:
            errors.append(f"{where}: needs exactly one `  - Depends on` bullet, has {e.depends_bullets}")
        if e.model is None:
            errors.append(f"{where}: needs a `  - Model: Opus|Fable, Planned|Not Planned.` bullet")
        for dep in e.depends:
            if dep == e.id:
                errors.append(f"{where}: depends on itself")
            elif rm.next_id is not None and int(dep[1:]) >= rm.next_id:
                errors.append(f"{where}: depends on {dep}, which was never issued")
    errors.extend(cycles(rm))
    return errors


def cycles(rm: Roadmap) -> list[str]:
    found: list[str] = []
    state: dict[str, str] = {}

    def visit(e: Entry, path: list[str]) -> None:
        if state.get(e.id) == "done":
            return
        if state.get(e.id) == "active":
            loop = path[path.index(e.id):] + [e.id]
            found.append("dependency cycle: " + " -> ".join(loop))
            return
        state[e.id] = "active"
        for dep in rm.prerequisites(e):
            visit(dep, path + [e.id])
        state[e.id] = "done"

    for e in rm.open():
        visit(e, [])
    return found


def reserve_ids(count: int, path: Path = ROADMAP) -> list[str]:
    text = path.read_text()
    m = NEXT_ID_RE.search(text)
    if m is None:
        sys.exit("roadmap: no `Next free ID: **R<n>**` line in the header")
    first = int(m["num"])
    text = text[: m.start()] + f"{m['pre']}{first + count}{m['post']}" + text[m.end():]
    path.write_text(text)
    return [f"R{n}" for n in range(first, first + count)]


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("list", "next"):
        s = sub.add_parser(name)
        s.add_argument("--track")
    for name in ("show", "deps"):
        sub.add_parser(name).add_argument("id")
    sub.add_parser("tracks")
    sub.add_parser("lint")
    sub.add_parser("new-id").add_argument("-n", "--count", type=int, default=1)
    args = p.parse_args()

    if args.cmd == "new-id":
        print("\n".join(reserve_ids(args.count)))
        return 0
    rm = parse()
    if args.cmd == "lint":
        errors = lint(rm)
        for err in errors:
            print(f"docs/roadmap.md: {err}", file=sys.stderr)
        return 1 if errors else 0
    if args.cmd == "tracks":
        for t in rm.tracks:
            count = sum(1 for e in rm.open() if e.track == t.slug and not e.standing)
            print(f"{t.slug or '?':<12}  {count:>3} open  {t.title}")
        return 0
    if args.cmd in ("show", "deps"):
        entry = rm.find(args.id)
        if args.cmd == "show":
            print(entry.body)
        else:
            for e in rm.work_order([entry])[:-1]:
                print(e.describe())
        return 0
    order = rm.work_order(rm.track_roots(args.track))
    for e in order[:1] if args.cmd == "next" else order:
        print(e.describe())
    return 0


def parse_entry(lines: list[str], i: int, m: re.Match, track: Track | None, out: list[Entry]) -> int:
    """Parse the entry starting at line `i`; return the index of the line after it."""
    title_parts = [m["rest"]]
    j = i + 1
    while "**" not in title_parts[-1] and j < len(lines) and lines[j].strip():
        title_parts.append(lines[j].strip())
        j += 1
    joined = " ".join(title_parts)
    title, _, after = joined.partition("**")
    k = i + 1
    while k < len(lines) and not re.match(r"^(- \[[ xX]\] |#{1,3} )", lines[k]):
        k += 1
    body = "\n".join(lines[i:k]).rstrip()
    mm = MODEL_RE.search(body)
    bullets = list(DEPENDS_RE.finditer(body))
    depends_text = " ".join(b["text"] for b in bullets)
    out.append(
        Entry(
            num=int(m["num"]),
            title=title.strip(),
            body=body,
            track=(track.slug if track else None) or "?",
            line=i + 1,
            checked=m["mark"] != " ",
            standing="*(standing" in after,
            model=mm["model"] if mm else None,
            planned=(mm["plan"] == "Planned") if mm else None,
            depends=[f"R{n}" for n in ID_RE.findall(depends_text)],
            depends_bullets=len(bullets),
            depends_on_track=ALL_IN_TRACK in " ".join(depends_text.split()),
        )
    )
    return k


if __name__ == "__main__":
    sys.exit(main())
