# PROBE: a `mut` parameter moved before a call to `abort(...)`.
#
# **Differs.** The pin runs it and prints `x`: `abort` never returns, so
# nothing reaches the function's exit with `s` moved. Mojito rejects it,
# "'s' is uninitialized at return from this function": no checked fact
# says the call never returns, so the body still falls through to its exit.
# Filed in `docs/roadmap.md` (R50). When Mojito runs it, promote this file
# to `assets/ownership_ok/` with its manifest row.
#
# Observed 2026-10-21 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run abort_after_mut_parameter_move.mojo
#         cargo run -- run conformance/probes/abort_after_mut_parameter_move.mojo
from std.os import abort


def take(var s: String):
    print(s)


def f(mut s: String):
    take(s^)
    abort("no")


def main():
    var s = String("x")
    if s.byte_length() > 5:
        f(s)
    print(s)
