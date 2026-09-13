# All six elementwise comparisons yield bool masks: signed and unsigned
# integer lanes compare by value, float lanes use the ordered predicates (a
# NaN lane compares False under every method, `ne` included), and a scalar
# operand splats. They are methods because upstream's infix `<` is
# `Scalar`-only and its `==` compares whole vectors. The NaN is made at run
# time: the pin folds a constant `0.0 / 0.0` receiver's `ne` to True.
def runtime_zero() -> Float64:
    var total = 0.0
    for i in range(3):
        total += Float64(i) - Float64(i)
    return total

def main():
    var i = SIMD[DType.int32, 4](-2, 0, 2, 2147483647)
    var j = SIMD[DType.int32, 4](2, 0, -2, -2147483648)
    print(i.lt(j), i.le(j), i.gt(j), i.ge(j), i.eq(j), i.ne(j))
    var u = SIMD[DType.uint8, 4](255, 0, 128, 127)
    print(u.lt(128), u.le(128), u.gt(128), u.ge(128), u.eq(255), u.ne(0))
    var s = SIMD[DType.int8, 4](-128, -1, 0, 127)
    print(s.lt(0), s.ge(0), s.eq(-1), s.lt(0))
    var f = SIMD[DType.float64, 4](1.0, -1.0, 0.0, 2.5)
    var g = SIMD[DType.float64, 4](1.0, 1.0, -0.0, 2.25)
    print(f.lt(g), f.le(g), f.gt(g), f.ge(g), f.eq(g), f.ne(g))
    var zero = SIMD[DType.float64, 4](runtime_zero())
    var nan = zero / zero
    print(nan.lt(f), nan.le(f), nan.gt(f), nan.ge(f), nan.eq(f), nan.ne(f), nan.eq(nan), nan.ne(nan))
    var h = SIMD[DType.float32, 4](0.1, 0.2, 0.3, 1.0)
    print(h.lt(0.25), h.eq(0.1), h.gt(SIMD[DType.float32, 4](0.05, 0.25, 0.3, 0.999)), h.ne(h))
    var u64 = SIMD[DType.uint64, 2](18446744073709551615, 9223372036854775808)
    print(u64.gt(1), u64.lt(9223372036854775807), u64.eq(18446744073709551615))
    var n = SIMD[DType.int, 2](-1, 1)
    print(n.lt(0), n.eq(1), n.ne(n))
