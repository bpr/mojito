# A module `comptime` alias of a typed `#kgen.simd` attribute is a constant
# of its `!kgen.scalar<ui8>` storage type, as upstream's `_mIsSigned` mask
# aliases in `std.builtin.dtype` are: it builds a `UInt8` and is a `pop`
# operand at that type.
comptime _mIsSigned = __mlir_attr.`#kgen.simd<1> : !kgen.scalar<ui8>`
comptime _mIsFloat = __mlir_attr.`#kgen.simd<64> : !kgen.scalar<ui8>`


def masked(code: UInt8) -> UInt8:
    return UInt8(mlir_value=__mlir_op.`pop.simd.and`(code._mlir_value, _mIsFloat))


def main():
    print(UInt8(mlir_value=_mIsSigned))
    print(masked(UInt8(77)), masked(UInt8(13)))
