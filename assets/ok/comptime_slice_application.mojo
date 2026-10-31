# A slice subscript of a compile-time value is upstream's `__getitem__` call
# on the slice literal's construction: at a parameter position, bound to a
# `comptime` local or module constant, under a ternary over a
# `dtype == .float64` condition (`_exp_taylor`'s spelling), and spelled out
# as the call itself.
from std.builtin.builtin_slice import ContiguousSlice
from std.math.polynomial import polynomial_evaluate

comptime C = List([Float64(1.0), 2.0, 3.0])
comptime S = C[1:]


def g[c: Span[Float64, _]]():
    print(len(c), c[0])


def taylor[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width]) -> SIMD[dtype, width]:
    comptime coefficients = List([Scalar[dtype](1.0), 1.0, 0.5, 0.25])
    return polynomial_evaluate[
        coefficients[:] if dtype == .float64 else coefficients[:2],
    ](x)


def main():
    comptime c = List([Float64(1.0), 2.0, 3.0])
    g[c[:2]]()
    g[c[1:]]()
    g[C[:2]]()
    g[S]()
    comptime s = c[1:]
    g[s]()
    print(taylor(Float64(2)), taylor(SIMD[DType.float32, 2](1, 2)))
    g[c.__getitem__(ContiguousSlice(None, 2, None))]()
    g[c.__getitem__(ContiguousSlice(Optional[Int](None), 2, None))]()
