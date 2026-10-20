# PROBE: a `Span(list)` view used after the list is cleared.
#
# The pin runs it and prints:
#   3
#
# Its `Span(list)` rebinds the list's interior pointer to `origin_of(list)`,
# so clearing the list does not invalidate the view. Mojito rejects it:
# "access to 'xs' conflicts with live reference 'ys'" (R1155).
#
# Observed 2026-10-19 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run span_of_list_survives_clear.mojo
#         cargo run -- run conformance/probes/span_of_list_survives_clear.mojo
def main():
    var xs: List[Int] = [1, 2, 3]
    var ys = Span(xs)
    xs.clear()
    print(len(ys))
