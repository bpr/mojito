# PROBE (divergence): a trivial rvalue into `var` beside a read overload.
#
# The pin calls `q(x + 1)` "ambiguous call to 'q'" while accepting `q(7)`,
# `q(x)` and `q(x^)` in the same program, so only the non-literal trivial
# rvalue is ambiguous. Mojito accepts all four and selects the `var` overload
# for `q(x + 1)`.
#
# Beside a type pack the pin follows a different rule — a bare literal is the
# ambiguous case there, and `x + 1` selects the `var` candidate
# (`pack_overload_var_trivial_undecided.mojo`). `ArgumentBinding`'s `undecided`
# bit models the pack rule and is deliberately kept off non-variadic
# candidates: applying it here would reject `q(7)`, which the pin accepts.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    rejected at `q(x + 1)`, "ambiguous call to 'q'"
#   mojito: 1, 1, 2, 1
#
# The place `q(x)` is settled: the implicit copy costs, and both select the
# read overload.
#
# Run:    mojo run overload_var_trivial_rvalue.mojo
#         cargo run -- run conformance/probes/overload_var_trivial_rvalue.mojo
#
# When answered: either a rank term reproduces the pin's line, and this file
# becomes an `assets/type_error/` fixture, or the divergence is kept and moves
# to `docs/non-goals.md`.
def q(var a: Int) -> Int:
    return 1


def q(a: Int) -> Int:
    return 2


def main():
    var x = 5
    print(q(7))
    print(q(x + 1))
    print(q(x))
    print(q(x^))
