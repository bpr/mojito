# `SIMD` declares `Roundable` with upstream's two `__round__` members, and
# the prelude's `round` is upstream's `std.math.round`.


def round_any[T: Roundable](x: T) -> T:
    return round(x)


def round_digits[T: Roundable](x: T) -> T:
    return x.__round__(1)


def main():
    var v = SIMD[DType.float32, 4](1.5, 2.5, -0.5, 3.7)
    print(v.__round__())
    print(round(v))
    print(Float64(2.5).__round__(), Float64(2.567).__round__(2))
    print(Float64(1234.5).__round__(-2))
    print(Int(1234).__round__(-2), UInt(1234).__round__(-2), Int(7).__round__(1))
    print(SIMD[DType.int32, 2](1234, -1255).__round__(-1))
    print(SIMD[DType.bool, 2](True, False).__round__(1))
    print(round(2.567, 2), round(2.5), round(-2.5))
    print(round(Int8(3)), round(UInt(5)), round(Int(1234), -2))
    print(round(Int8(-3), -1))
    print(round_any(Float32(2.5)), round_any(Int(3)))
    print(round_any(SIMD[DType.float64, 2](0.4, 1.6)))
    print(round_digits(2.567))
    comptime V = SIMD[DType.int32, 2](1234, -1255).__round__(-1)
    comptime R = round(Float64(2.567), 2)
    print(V, R)
