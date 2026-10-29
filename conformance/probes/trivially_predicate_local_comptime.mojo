# PROBE: a local `comptime` bound to a builtin `IsTrivially*` predicate.
#
# **Differs.** The pin runs it and prints `True`. Mojito rejects it, "a
# generic comptime alias must be declared at module scope": the application
# parses as a type application, so the local binding takes the alias route,
# and the value route types `IsTriviallyMovable[Int]` as a type. Filed in
# `docs/roadmap.md` (R1619). When Mojito runs it, promote this file to
# `assets/ok/`.
#
# Observed 2026-10-29 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run trivially_predicate_local_comptime.mojo

from std.traits import IsTriviallyMovable


def main():
    comptime trivially = IsTriviallyMovable[Int]
    print(trivially)
