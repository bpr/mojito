# `if case`/`elif case` evaluate each right-hand side only when reached;
# `while case` re-evaluates per iteration and runs `else` on failure; a
# sequence pattern runs as an `if case` condition; an irrefutable
# `if case x = n` is accepted without a diagnostic.
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
