# PROBE: `List(iterable)` owns a copy of a slice's elements.
#
# The pin runs it and prints:
#   [1, 2, 9] [0, 1, 2, 3]
#
# Mojito rejects it: "invalid call to 'List': no constructor overload
# matches the supplied arguments" (R1154).
#
# Observed 2026-10-19 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run list_from_iterable.mojo
#         cargo run -- run conformance/probes/list_from_iterable.mojo
def main():
    var xs: List[Int] = [0, 1, 2, 3]
    var ys = List(xs[1:3])
    ys.append(9)
    print(ys, xs)
