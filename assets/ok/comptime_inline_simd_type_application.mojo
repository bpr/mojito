# A parameter constant whose type embeds a compile-time application
# (`SIMD[_integral_type_of[dtype](), width]`) folds that application with
# the value's own.
from std.sys.info import bit_width_of
from std.builtin.dtype import _integral_type_of


def magnitude_mask[dtype: DType, width: SIMDLength](x: SIMD[dtype, width]):
    comptime m = SIMD[_integral_type_of[dtype](), width](
        (1 << (bit_width_of[dtype]() - 1)) - 1
    )
    print(m)


def integral_max[dtype: DType, width: SIMDLength](x: SIMD[dtype, width]):
    comptime k = SIMD[_integral_type_of[dtype](), width].MAX
    print(k)


def alias_max[dtype: DType, width: SIMDLength](x: SIMD[dtype, width]):
    comptime TInt = SIMD[_integral_type_of[dtype](), width]
    comptime n = TInt(3)[0]
    comptime k = TInt.MAX
    print(n, k)


def main():
    magnitude_mask(SIMD[DType.float32, 4](0, -0.0, 1, 2))
    integral_max(SIMD[DType.float32, 4](0, -0.0, 1, 2))
    alias_max(SIMD[DType.float32, 4](0, -0.0, 1, 2))
