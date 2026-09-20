# PROBE (open question): what rule does the pin follow for a trivial value
# handed to a `var` parameter beside a pack?
#
# Two type-pack overloads differ by one regular `var b: Int` parameter. For an
# argument that is not a place the pin's selection follows no rule the probes
# recovered, so Mojito reports every such call ambiguous. That is a safe
# over-rejection: the pin selects an overload for four of these five calls.
#
# Observed 2026-09-19 against `Mojo 1.1.0.dev2026082605 (dd957314)`, one call at a time:
#   g(x, x + 1, x)   pin: 3           mojito: ambiguous
#   g(x, Int(2), x)  pin: 2           mojito: ambiguous
#   g(x, v^, x)      pin: 2           mojito: ambiguous
#   g(x, True, x)    pin: 2 (`var b: Bool`)   mojito: ambiguous
#   g(x, 2, x)       pin: ambiguous   mojito: ambiguous
# A place argument (`g(x, v, x)`) is settled: the implicit copy costs, the pin
# prints 2, and so does Mojito.
#
# Run:    mojo run pack_overload_var_trivial_undecided.mojo
#         cargo run -- run conformance/probes/pack_overload_var_trivial_undecided.mojo
#
# If a rule is found: teach `VariadicBinding::bind`
# (`checker/overload_support.rs`) to rank the case instead of marking it
# undecided, and move the calls into `assets/ok/`.
def g[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 2


def g[*Ts: Writable](a: Int, var b: Int, *rest: *Ts) -> Int:
    return 3


def main():
    var x = 1
    print(g(x, x + 1, x))
