# PROBE (re-probe): does a `comptime for` in a `finally` copied before two
# exits compile?
#
# **Differs.** Found while landing R884 (2026-10-22) against Mojo
# 1.2.0.dev2026101705 (46f49943). Upstream's `lowerTryFinally` pastes the
# `finally` before the `continue` and at the `else`'s `try.yield`; with the
# `comptime for` in it, the pin rejects the program, "use of uninitialized
# value '(expression temporary)'" at `print("k", k)`, then "failed to run
# the pass manager". One copy compiles (the same `finally` with no
# `continue` prints `k 0`, `k 1` per iteration). Mojito runs it, printing
# `k 0`, `k 1` twice and `end`, which is what the source says. When the pin
# runs it, promote it to `assets/ok/`.
#
# Run:    mojo run finally_copy_comptime_for.mojo
#         cargo run -- run conformance/probes/finally_copy_comptime_for.mojo


def main():
    for i in range(2):
        try:
            if i == 0:
                continue
        finally:
            comptime for k in range(2):
                print("k", k)
    print("end")
