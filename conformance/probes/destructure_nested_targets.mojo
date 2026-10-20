# PROBE: nested tuple targets mixing places, `var`, `ref`, and `_` at every depth, including `var (p, q), r = (7, 8), 9`.
#
# The pin runs it and prints:
#   1 2 3
#   4 5 6
#   7 8 9
#   1 2 3
#   1 2
#   1 s
#   9
# Diagnostics:
#   warning at line 2: assignment to 'a' was never used; assign to '_' instead?
#   warning at line 3: assignment to 'b' was never used; assign to '_' instead?
#   warning at line 4: assignment to 'c' was never used; assign to '_' instead?
#
# Observed 2026-10-17 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects the nominal subscript target `xs[i]` until R1142 lands (`assets/ok/destructure_nested_targets.mojo` holds the rest).
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run destructure_nested_targets.mojo
#         cargo run -- run conformance/probes/destructure_nested_targets.mojo
def main():
    var a = 0
    var b = 0
    var c = 0
    (a, (b, c)) = (1, (2, 3))
    print(a, b, c)
    var (x, (y, z)) = (4, (5, 6))
    print(x, y, z)
    var (p, q), r = (7, 8), 9
    print(p, q, r)
    var xs: List[Int] = [0, 0]
    (xs[0], (xs[1], a)) = (1, (2, 3))
    print(xs[0], xs[1], a)
    (var m, ref n) = (xs[0], xs[1])
    print(m, n)
    var t = (1, "s")
    var (i, s) = t
    print(i, s)
    _, (b, _) = (0, (9, 0))
    print(b)
