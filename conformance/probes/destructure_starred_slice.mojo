# PROBE: `[first, *rest]` destructuring and `[1, *mid, 5]` / `[*_, 5]` / `[*all]` patterns over a List; the remainder is a borrowing `Span[Int, origin_of(values["element"])]`.
#
# The pin runs it and prints:
#   1 4
#   4 5
#   3
#   ends 5
#   5
#   error: list pattern does not match value length
# Diagnostics:
#   warning at line 42: assignment to 'c' was never used; assign to '_' instead?
#   warning at line 42: assignment to 'd' was never used; assign to '_' instead?
#
# Observed 2026-10-17 against `Mojo 1.2.0.dev2026101705 (46f49943)`; the
# too-short starred target, moved here from R1136's probe, re-run 2026-10-20.
# Mojito: rejects starred targets until R1137 lands.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run destructure_starred_slice.mojo
#         cargo run -- run conformance/probes/destructure_starred_slice.mojo
def main() raises:
    var values: List[Int] = [1, 2, 3, 4, 5]
    var [first, *rest] = values
    print(first, len(rest))
    var [*lead, last] = values
    print(len(lead), last)
    match values:
        case [1, *mid, 5]:
            print(len(mid))
        case _:
            print("no")
    match values:
        case [1, *_, 4]:
            print("yes")
        case [*_, 5]:
            print("ends 5")
    match values:
        case [*all]:
            print(len(all))
    var three: List[Int] = [1, 2, 3]
    try:
        var [a, *_, b, c, d] = three
        print(a, b)
    except e:
        print("error:", e)
