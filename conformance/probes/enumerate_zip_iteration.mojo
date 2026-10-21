# PROBE: `enumerate` and `zip` yield tuples a loop target destructures.
#
# The pin runs it and prints:
#   0 a
#   1 b
#   a 10
#   b 20
#
# Mojito: "Undefined variable 'enumerate'"; the bundled `std.iter` has
# neither function (R1229).
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
#
# Run:    mojo run enumerate_zip_iteration.mojo
#         cargo run -- run conformance/probes/enumerate_zip_iteration.mojo
def main():
    var xs: List[String] = ["a", "b"]
    var ys: List[Int] = [10, 20]
    for i, x in enumerate(xs):
        print(i, x)
    for x, y in zip(xs, ys):
        print(x, y)
