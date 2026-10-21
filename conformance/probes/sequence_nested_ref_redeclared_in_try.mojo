# PROBE: a sequence store with a nested tuple element of `ref` leaves, then a `try` that redeclares the same leaf names.
#
# The pin runs it and prints:
#   1 2
#   5 6
#
# Mojito: MIR verification fails, "drop.var of slot ... ('$elem...') that
# every path has already ended" (R1232).
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run sequence_nested_ref_redeclared_in_try.mojo
#         cargo run -- run conformance/probes/sequence_nested_ref_redeclared_in_try.mojo
def main() raises:
    var t: List[Tuple[Int, Int]] = [(1, 2), (3, 4)]
    if len(t) > 0:
        [(ref a, ref b), _] = t
        print(a, b)
    try:
        var s: List[Int] = [5, 6]
        [ref a, ref b] = s
        print(a, b)
    except e:
        print(e)
