# A local `comptime` binding constructed through a local type alias over the
# function's binders is lifted over every binder the alias names.
from std.sys.info import bit_width_of
from std.builtin.dtype import _integral_type_of


def magnitude_mask[dtype: DType, width: SIMDLength](x: SIMD[dtype, width]):
    comptime TInt = SIMD[_integral_type_of[dtype](), width]
    comptime m = TInt((1 << (bit_width_of[dtype]() - 1)) - 1)
    print(m)


def is_zero_bits[
    dtype: DType, width: SIMDLength
](x: SIMD[dtype, width]) -> SIMD[DType.bool, width]:
    comptime TInt = SIMD[_integral_type_of[dtype](), width]
    comptime magnitude = TInt((1 << (bit_width_of[dtype]() - 1)) - 1)
    return (x._to_bits_signed() & magnitude).eq(0)


def first_lane[dtype: DType, width: SIMDLength](x: SIMD[dtype, width]):
    comptime TInt = SIMD[_integral_type_of[dtype](), width]
    comptime n = TInt(3)[0]
    print(n)


def main():
    magnitude_mask(SIMD[DType.float32, 4](0, -0.0, 1, 2))
    print(is_zero_bits(SIMD[DType.float32, 4](0, -0.0, 1, 2)))
    print(is_zero_bits(SIMD[DType.float64, 2](0, 3)))
    first_lane(SIMD[DType.float32, 4](0, -0.0, 1, 2))
