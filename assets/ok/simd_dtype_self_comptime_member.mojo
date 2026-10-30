# A struct's `comptime` member names a `SIMD` dtype slot
# (`Scalar[Self.integral_type]`), closed or symbolic, as upstream's
# `FPUtils` spells its bit types.
from std.builtin.dtype import _integral_type_of


struct Bits[dtype: DType]:
    comptime integral_type = _integral_type_of[Self.dtype]()
    comptime wide = DType.int64

    @staticmethod
    def zero() -> Scalar[Self.integral_type]:
        return Scalar[Self.integral_type](0)

    @staticmethod
    def widen(value: Int) -> SIMD[Self.wide, 2]:
        return SIMD[Self.wide, 2](value)


def main():
    print(Bits[DType.float32].zero(), Bits[DType.float64].widen(3))
    comptime T = Bits[DType.float16].integral_type
    print(T)
