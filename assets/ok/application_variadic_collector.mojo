# A compile-time call applies a function with a positional collector, as at
# the pin: the call is the callee's application with its arguments flat
# (`total(n, Int(1))`), which the elaborator runs with the run as the one
# `VariadicList` a run-time call passes. A read and an owned collector, a
# keyword parameter past the collector, a `comptime` binding, and a
# `StaticTuple` built by its `*elems` initializer as a parameter.
from std.utils import StaticTuple


def total(*xs: Int) -> Int:
    var s = 0
    for x in xs:
        s += x
    return s


def owned_total(var *xs: Int) -> Int:
    var s = 0
    for x in xs:
        s += x
    return s


def scaled(*xs: Int, scale: Int = 1) -> Int:
    var s = 0
    for x in xs:
        s += x
    return s * scale


def lanes[n: Int](
    x: SIMD[DType.int8, total(n, 1)]
) -> SIMD[DType.int8, total(n, 1)]:
    return x


def owned_lanes[n: Int](
    x: SIMD[DType.int8, owned_total(n, 1)]
) -> SIMD[DType.int8, owned_total(n, 1)]:
    return x


def scaled_lanes[n: Int](
    x: SIMD[DType.int8, scaled(n, 1, scale=2)]
) -> SIMD[DType.int8, scaled(n, 1, scale=2)]:
    return x


def k[n: Int]() -> Int:
    return n


def gather[
    n: SIMDLength, //, mask: StaticTuple[SIMDLength, n]
](v: SIMD[DType.int32, n]) -> Int:
    return Int(mask[0])


def main():
    print(lanes[1](SIMD[DType.int8, total(1, 1)](1)))
    print(owned_lanes[1](SIMD[DType.int8, owned_total(1, 1)](2)))
    print(scaled_lanes[1](SIMD[DType.int8, scaled(1, 1, scale=2)](3)))
    comptime sum = total(1, 2, 3)
    print(sum)
    print(k[total(4, 5)]())
    print(k[total()]())
    var v = SIMD[DType.int32, 2](5, 6)
    print(gather[mask=StaticTuple[SIMDLength, 2](1, 0)](v))
