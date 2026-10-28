# `SIMD`'s `__abs__`, `__floor__`, `__ceil__`, `__trunc__`,
# `is_power_of_two`, `select`, and `cast` are upstream's library bodies, and
# `SIMD` declares `Absable`, `Ceilable`, `Floorable`, and `Truncable`, so a
# vector or a sized scalar binds those bounds and `bool` casts run, as at
# the pin.
from std.math import Absable, Ceilable, Floorable, Truncable


def ab[T: Absable](x: T) -> T:
    return x.__abs__()


def fl[T: Floorable](x: T) -> T:
    return x.__floor__()


def main():
    var v = SIMD[DType.int32, 4](1, -2, 3, 8)
    var f = SIMD[DType.float32, 4](1.5, -2.5, 3.25, -0.5)
    var m = v.gt(0)
    print(v.__abs__(), f.__abs__(), f.__floor__(), f.__ceil__(), f.__trunc__())
    print(Int32(-3).__abs__(), Float32(2.5).__floor__(), Float64(-2.5).__trunc__())
    print(
        ab(Float32(-1.5)),
        ab(SIMD[DType.int32, 4](-1, 2, -3, 4)),
        fl(Int32(3)),
        fl(Float32(3.5)),
    )
    print(
        conforms_to(SIMD[DType.int32, 4], Absable),
        conforms_to(Float32, Ceilable),
        conforms_to(UInt8, Truncable),
    )
    print(v.is_power_of_two(), UInt8(64).is_power_of_two())
    print(m.select(v, SIMD[DType.int32, 4](0)), m.select(v, 0))
    print(m.select[DType.float32](f, f * 2))
    print(v.cast[DType.float32](), f.cast[DType.int32]())
    print(Int32(5).cast[DType.float64](), UInt8(255).cast[DType.int8]())
    print(v.cast[DType.bool](), f.cast[DType.bool](), m.cast[DType.int8]())
    comptime c = SIMD[DType.int8, 2](3, -4).cast[DType.float32]()
    print(c, Int(Float64(3.7)), Int8(UInt8(200)), UInt8(Int8(-1)))
