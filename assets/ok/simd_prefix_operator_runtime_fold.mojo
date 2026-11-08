# A run-time prefix operator over a compile-time `SIMD` vector folds its
# builtin dunder where it is spelled, as the pinned Mojo's parser folds a
# builtin call over parameter operands, so `__neg__`'s and `__pos__`'s
# numeric `comptime assert` is never judged: `-b` over a `bool` mask is its
# inversion. A numeric operand folds as before, and an infix `+` or `-`
# over a mask folds as `i1` arithmetic.

comptime b = SIMD[DType.bool, 4](True, False, True, False)
comptime a = SIMD[DType.int32, 4](1, 2, 3, 4)
comptime c = -b


def main():
    var x = -b
    print(x)
    print(~b)
    print(+b)
    print(c)
    print(~a)
    print(-a)
    print(+a)
    print(-SIMD[DType.int8, 2](3))
    print(b + b)
    print(b - a.eq(a))
    var r = SIMD[DType.int32, 2](5, 6)
    print(-r)
