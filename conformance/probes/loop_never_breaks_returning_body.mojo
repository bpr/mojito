# PROBE: a value-returning body whose last statement is a loop that never
# breaks.
#
# **Differs.** The pin runs it and prints `4`: `LowerSemanticCF` marks the
# `while True` condition's else arm dead, so `lowerLITLoop` follows the loop
# with `hlcf.unreachable` and no path reaches the end of `h`'s `else` arm.
# Mojito rejects it, "'h' does not return a value on every path": the
# checker's return analysis (`stmt_returns`) knows no `while` that never
# breaks. Filed in `docs/roadmap.md` (R1242). When Mojito runs it, promote
# this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-10-30 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run loop_never_breaks_returning_body.mojo
#         cargo run -- run conformance/probes/loop_never_breaks_returning_body.mojo
def h[n: Int](x: Int) -> Int:
    comptime if n == 1:
        return x
    else:
        while True:
            pass


def main():
    print(h[1](4))
