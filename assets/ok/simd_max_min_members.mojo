# `SIMD`'s `MAX`, `MIN`, `MAX_FINITE`, and `MIN_FINITE` are upstream's
# `comptime` members over `std.utils.numerics`: read on a `SIMD`
# application, a sized alias, `Int`/`UInt`, a `comptime` alias, `type_of`,
# and `Scalar[dt]` in a generic, in a `comptime` binding, a `comptime if`,
# and a parameter argument, and the numerics functions called directly.
# Float32's finite limits are compared rather than printed, since float
# text differs from the pin's (R159).
from std.utils.numerics import (
    inf,
    max_finite,
    max_or_inf,
    min_finite,
    min_or_neg_inf,
    neg_inf,
)


@fieldwise_init
struct Tag[v: Int8]:
    pass


def top[dt: DType]() -> Scalar[dt]:
    return Scalar[dt].MAX


def lo[dt: DType, n: SIMDLength](x: SIMD[dt, n]) -> SIMD[dt, n]:
    return type_of(x).MIN


def main():
    print(SIMD[DType.int32, 1].MAX)
    print(
        Float32.MAX,
        Float32.MIN,
        Float32.MAX_FINITE == Float32(3.4028235e38),
        Float32.MIN_FINITE == -Float32(3.4028235e38),
    )
    print(Int.MAX, Int.MIN, UInt8.MAX, UInt8.MIN, Int8.MIN_FINITE)
    print(SIMD[DType.int16, 4].MAX)
    print(Float16.MAX_FINITE, Scalar[DType.bool].MAX, Scalar[DType.bool].MIN)
    print(UInt.MAX, Int64.MAX, Int32.MIN, UInt64.MAX)
    comptime T = Int
    print(T.MAX)
    var v = Float32(1)
    print(type_of(v).MAX, v)
    print(SIMD[DType.float32, 2].MIN)
    comptime m = UInt16.MAX
    print(m)
    print(top[DType.uint16](), top[DType.float32]())
    print(lo(SIMD[DType.int8, 2](1)))
    comptime if Int8.MAX == 127:
        print("yes")
    var t = Tag[Int8.MAX]()
    _ = t^
    print(Int.MAX - 1, UInt8.MAX // 2)
    var x = Int32(5)
    print(min(x, Int32.MAX))
    print(
        max_finite[DType.uint64](),
        min_finite[DType.int64](),
        max_or_inf[DType.uint8](),
        min_or_neg_inf[DType.float64](),
    )
    print(inf[DType.float64](), neg_inf[DType.float16]())
    print(max_finite[DType.bool](), min_finite[DType.float16]())
