# PROBE: `if case`/`elif case` evaluate each right-hand side only when reached; `while case` re-evaluates per iteration and runs `else` on failure; an irrefutable `if case x = n` is accepted without a diagnostic.
#
# The pin runs it and prints:
#   get 1
#   x 1
#   get 1
#   get 3
#   y 0
#   get 3
#   k 3
#   get 2
#   k 2
#   get 1
#   k 1
#   1 2
#   irrefutable 0
#
# Observed 2026-10-17 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: runs every part but the `[a, b]` sequence pattern (R1135);
# `assets/ok/if_case_elif_chains_and_while_case.mojo` covers the rest.
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run if_case_and_while_case.mojo
#         cargo run -- run conformance/probes/if_case_and_while_case.mojo
def get(i: Int) -> Tuple[Int, Int]:
    print("get", i)
    return (i, 0)


def main():
    if case (var x, 0) = get(1):
        print("x", x)
    elif case (3, var y) = get(3):
        print("y", y)
    else:
        print("else")
    if case (2, var x) = get(1):
        print("x", x)
    elif case (3, var y) = get(3):
        print("y", y)
    else:
        print("else")
    var n = 3
    while case (var k, 0) = get(n):
        print("k", k)
        n -= 1
        if n == 0:
            break
    else:
        print("loop else")
    var values: List[Int] = [1, 2]
    if case [a, b] = values:
        print(a, b)
    if case x = n:
        print("irrefutable", x)
