# A tuple index over an `Int`-typed compile-time expression selects its
# element: a module constant's operation (`t[N + 1]`), and an annotated
# local constant read alone or in an operation (`t[n]`, `t[n + 1]`),
# reading and storing through the subscript.
comptime N: Int = 3


def main():
    var t = (1, "two", 3.5, True, 5)
    print(t[N - 2])
    print(t[N + 1])
    comptime n: Int = 1
    print(t[n])
    print(t[n + 1])
    t[N - 3] = 42
    t[N + 1] += 9
    print(t[0], t[4])
    comptime lx: Int = 7
    comptime lg = lx // 0
    print(lg, lx % 2)
