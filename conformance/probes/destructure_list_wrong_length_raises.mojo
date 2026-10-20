# PROBE: a fixed-length list destructuring raises `list pattern does not match value length` before any store, so a `raises` context is required.
#
# The pin runs it and prints:
#   error: list pattern does not match value length
#   error: list pattern does not match value length
#   error: list pattern does not match value length
#   1 2 3
#   error: list pattern does not match value length 1 2
# Diagnostics:
#   warning at line 14: assignment to 'rest' was never used; assign to '_' instead?
#   warning at line 14: assignment to 'c' was never used; assign to '_' instead?
#   warning at line 14: assignment to 'd' was never used; assign to '_' instead?
#   warning at line 21: assignment to 'z' was never used; assign to '_' instead?
#   warning at line 24: assignment to 'x' was never used; assign to '_' instead?
#   warning at line 24: assignment to 'y' was never used; assign to '_' instead?
#
# Observed 2026-10-17 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects sequence targets until R1136 lands.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run destructure_list_wrong_length_raises.mojo
#         cargo run -- run conformance/probes/destructure_list_wrong_length_raises.mojo
def main():
    var values: List[Int] = [1, 2, 3]
    try:
        var [a, b] = values
        print(a, b)
    except e:
        print("error:", e)
    try:
        var [a, b, c, d] = values
        print(a, b, c, d)
    except e:
        print("error:", e)
    try:
        var [a, *rest, b, c, d] = values
        print(a, b)
    except e:
        print("error:", e)
    var x = 0
    var y = 0
    try:
        var z = 0
        [x, y, z] = values
        print(x, y, z)
        [x, y] = values
        print("unreachable")
    except e:
        print("error:", e, x, y)
