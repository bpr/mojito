# `std.simd`'s `_floor`, `_modf_scalar`, and `_modf`: upstream's bit-mask
# `_floor` over `float64` and a four-lane `float32` vector (its `0.25` is
# `0.125` at the pin too), `_modf_scalar` on positive and negative scalars,
# `_modf` over a `float64` vector and an integral scalar, and compile-time
# calls.
from std.simd import _floor, _modf, _modf_scalar


def main():
    print(_floor(Float64(2.5)), _floor(Float64(-1.5)))
    print(_floor(SIMD[DType.float32, 4](1.5, -0.5, 0.25, 1e10)))
    var t = _modf_scalar(Float64(3.25))
    print(t[0], t[1])
    var u = _modf_scalar(Float64(-3.25))
    print(u[0], u[1])
    var v = _modf(SIMD[DType.float64, 2](1.5, -2.75))
    print(v[0], v[1])
    var w = _modf(Int32(5))
    print(w[0], w[1])
    comptime f = _floor(Float32(7.75))
    comptime c = _modf_scalar(Float64(2.5))
    print(f, c[0], c[1])
