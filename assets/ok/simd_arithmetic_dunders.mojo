# Pinned against Mojo 1.2.0.dev2026101705 (2026-10-28).
# `+`, `-`, `*`, and prefix `-` over a `SIMD`, scalars included, are its
# declared `__add__`, `__sub__`, `__mul__`, and `__neg__`, and a literal or
# scalar left operand reaches the right one's `__radd__`, `__rsub__`, or
# `__rmul__`, as at the pin. The dunders are callable by name, `__pos__`
# included, and run in generic bodies over a symbolic dtype and width.


def square_sum[dt: DType, n: SIMDLength](v: SIMD[dt, n]) -> SIMD[dt, n]:
    return v * v + v - v


def offset[dt: DType](a: Scalar[dt]) -> Scalar[dt]:
    return a + 1.5


def scaled[dt: DType](a: Scalar[dt]) -> Scalar[dt]:
    return 2 * a - 1


def main():
    var a = 7
    var b = 2
    print(a + b, a - b, a * b, -a, 1 + a, a + 1, 10 - a, 3 * a)
    var f = Float64(7.5)
    print(f + 1, 2.5 + f, -f, f * f)
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(v + 1, 1 + v, v * v, -v)
    print(v + Int32(1), Int32(1) + v, Int32(10) - v)
    print(Int8(127) + 1, UInt(3) - 1, UInt(3) * 2)
    print(a.__add__(2), a.__radd__(1), a.__sub__(10), a.__rsub__(10))
    print(a.__mul__(3), a.__rmul__(3), a.__pos__(), a.__neg__())
    print(v.__neg__(), v.__pos__(), f.__neg__())
    print(square_sum(v), square_sum(Float32(1.5)))
    print(offset(Float32(1.0)), scaled(Int16(4)))
    var s = 0
    for i in range(5):
        s = s + i * a - 1
    print(s)
