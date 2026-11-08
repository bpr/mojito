# An annotated `comptime` literal binds at its declared type: a sized scalar
# keeps its dtype, a vector annotation splats the literal across its lanes,
# and a `Float64` annotation reads an integer literal as a float. A constant
# folded from a sized one wraps to its width.
comptime ONE: Int32 = 1
comptime ONES: SIMD[DType.int32, 4] = 1
comptime HALF: Float32 = 0.5
comptime TWO: Float64 = 2
comptime BYTE: UInt8 = 255
comptime COUNT: Int = 3
comptime SMALL: UInt8 = 5
comptime WRAPPED = SMALL + 254
comptime NEGATED = -SMALL
comptime SHIFTED = SMALL << 6


def scaled[n: Int](x: Int32) -> Int32:
    comptime factor: Int16 = 300
    print(factor.dtype, factor)
    return x * Int32(n)


def main():
    print(ONE.dtype, ONE)
    print(ONES)
    print(HALF.dtype, HALF)
    print(TWO)
    print(BYTE.dtype, BYTE)
    var widened = ONE + 1
    print(widened.dtype, widened)
    print(ONES + SIMD[DType.int32, 4](1, 2, 3, 4))
    print(scaled[COUNT](ONE))
    comptime local: Int64 = 7
    print(local.dtype, local)
    print(WRAPPED, NEGATED, SHIFTED)
