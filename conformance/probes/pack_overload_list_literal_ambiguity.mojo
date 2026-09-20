# PROBE (divergence): a list literal beside a `List` parameter and a pack.
#
# The pin calls this ambiguous, as it does for any argument that binds by
# reference either way. Mojito counts the literal's conversion to `List[Int]`
# against the second overload only, so it selects the first and prints 2: it
# accepts a program the pin rejects. A string literal is already costed on the
# pack side (`pack_element_conversion_count`); a list literal is not, because
# the regular binding's own cost is not the single conversion a pack-side
# charge of one would match.
#
# Observed 2026-09-19 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    error: ambiguous call to 'g'
#   mojito: prints 2
#
# Run:    mojo run pack_overload_list_literal_ambiguity.mojo
#         cargo run -- run conformance/probes/pack_overload_list_literal_ambiguity.mojo
#
# When fixed: move this file to `assets/type_error/` with its
# `conformance/assets-mojo-errors.tsv` row.
def g[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 2


def g[*Ts: Writable](a: Int, b: List[Int], *rest: *Ts) -> Int:
    return 3


def main():
    var x = 1
    print(g(x, [1, 2], x))
