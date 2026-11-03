# An `Int`-annotated `comptime` constant is the `Int` its annotation
# converts the literal to, at module scope as in a body: a compile-time
# read folds with `Int`'s operators, so `/` floors and `//` or `%` by zero
# folds to `0`, while an unannotated literal pair still divides exactly.
comptime N: Int = 3
comptime x: Int = 7
comptime y = x / 2
comptime g = x // 0
comptime h = x % 0
comptime M: Int = N + 1
comptime L = 7 / 2


def half(n: Int) -> Int:
    return n // 2


comptime Q = half(x) / 2


def main():
    print(N / 2, y, g, h, M, L, Q)
    comptime n: Int = 3
    comptime lx: Int = 7
    comptime ly = lx / 2
    comptime lg = lx // 0
    comptime lh = lx % 0
    print(n / 2, ly, lg, lh)
