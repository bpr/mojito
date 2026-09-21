# PROBE: a value-returning body whose last statement is `abort(...)`.
#
# **Differs.** The pin runs it and prints 1: `abort` never returns, so the
# body has no fall-through path. Mojito rejects it, "'f' does not return a
# value on every path": its return analysis knows only `return`, `raise`, and
# the compiler crossing `_mojito_abort`, not a call to the bundled
# `std.os.abort` that wraps it. Filed in `docs/roadmap.md` §3. When Mojito
# runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run abort_ends_a_returning_body.mojo
#         cargo run -- run conformance/probes/abort_ends_a_returning_body.mojo
from std.os import abort


def f(x: Int) -> Int:
    if x > 0:
        return 1
    abort("no")


def main():
    print(f(1))
