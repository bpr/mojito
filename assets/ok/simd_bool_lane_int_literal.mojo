# An integer literal converts at a `bool` lane, as the pin's
# `#pop.int_literal_convert` does: the lane is whether the literal is
# nonzero. So `~` over a mask (upstream's `self ^ -1`) inverts it, a literal
# operand or annotation converts, and a `Scalar[DType.bool]` or bool-vector
# value parameter binds a `Bool` or an integer literal.
def g[m: Scalar[DType.bool]]() -> Scalar[DType.bool]:
    return ~m

def h[m: SIMD[DType.bool, 2]]() -> SIMD[DType.bool, 2]:
    return ~m

def main():
    var m = Scalar[DType.bool](True)
    print(m ^ 1)
    print(m == 0)
    var x: Scalar[DType.bool] = 2
    print(x)
    print(SIMD[DType.bool, 2](1, 0))
    print(SIMD[DType.bool, 4](0, 2, -1, 0))
    comptime c = SIMD[DType.bool, 4](0, 2, -1, 0)
    print(c)
    comptime k: Scalar[DType.bool] = 4
    print(k)
    print(g[True]())
    print(g[0]())
    print(h[SIMD[DType.bool, 2](True, False)]())
    print(h[1]())
