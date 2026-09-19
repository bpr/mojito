# PROBE (open question): why does the pinned Mojo call this ambiguous?
#
# Two type-pack overloads differ by one extra regular parameter before the
# collector. With that parameter an `Int`, the pin selects the overload that
# binds it (`assets/ok/pack_overload_arity.mojo`). With it a `String`, the pin
# reports "ambiguous call to 'g'" for the same call shape, whether the
# arguments are literals or variables. Mojito ranks both the same way and
# selects the second overload, so it accepts a program the pin rejects.
#
# Observed 2026-09-19 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    error: ambiguous call to 'g'
#   mojito: prints 3
#
# Run:    mojo run pack_overload_string_regular_ambiguity.mojo
#         cargo run -- run conformance/probes/pack_overload_string_regular_ambiguity.mojo
#
# If the rule turns out to be the regular parameter's type class (a
# memory-only type against a pack element): teach
# `variadic_absorption_rank` (`checker/overload_support.rs`) to withhold its
# tie-break there, move this file to `assets/type_error/`, and add its
# `conformance/assets-mojo-errors.tsv` row.
# If a later pin accepts it and prints 3: promote it to `assets/ok/`.
def g[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 2


def g[*Ts: Writable](a: Int, b: String, *rest: *Ts) -> Int:
    return 3


def main():
    var x = 1
    var s = String("s")
    print(g(x, s, x))
