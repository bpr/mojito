# PROBE: a `mut` parameter moved before a loop whose body never breaks.
#
# **Differs.** The pin runs it and prints `x`: `LowerSemanticCF` marks the
# `while True` condition's else arm dead (`markRegionDeadDueToConstantCond`),
# so the body never breaks and `lowerLITLoop` follows the loop with
# `hlcf.unreachable`, at which `CheckLifetimes` checks nothing. Mojito
# rejects it, "'s' is uninitialized at return from this function": no pass
# marks the constant condition's arm dead, so the loop's exit stays live.
# Filed in `docs/roadmap.md` (R1242). When Mojito runs it, promote this file
# to `assets/ownership_ok/` with its manifest row.
#
# Observed 2026-10-21 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run loop_never_breaks_mut_parameter.mojo
#         cargo run -- run conformance/probes/loop_never_breaks_mut_parameter.mojo
def take(var s: String):
    print(s)


def f(mut s: String):
    take(s^)
    while True:
        pass


def main():
    var s = String("x")
    if s.byte_length() > 5:
        f(s)
    print(s)
