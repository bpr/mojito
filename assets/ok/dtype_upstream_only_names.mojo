# Every member of upstream's `struct DType` is a value: the wide and
# sub-byte integers, the low-precision float formats, and `bfloat16` print,
# compare, hash, answer their predicates and the float queries, and parse
# from their names, at run time and folded at compile time.


def code[dtype: DType]() -> Int:
    return Int(UInt8(mlir_value=dtype._as_ui8()))


def main():
    print(DType.uint128, DType.int128, DType.uint256, DType.int256)
    print(DType.bfloat16, DType.float4_e2m1fn, DType.float6_e2m3fn)
    print(DType.float6_e3m2fn, DType.float8_e8m0fnu, DType.float8_e3m4)
    print(DType.float8_e4m3fn, DType.float8_e4m3fnuz, DType.float8_e5m2)
    print(DType.float8_e5m2fnuz)
    # The sub-byte integers have no `write_to` arm upstream.
    print(DType._uint1, DType._uint2, DType._uint4)
    print(repr(DType.uint128), repr(DType.float8_e4m3fn))
    print(code[DType._uint1](), code[DType._uint4](), code[DType.uint128]())
    print(code[DType.int256](), code[DType.float4_e2m1fn]())
    print(code[DType.float8_e5m2fnuz](), code[DType.bfloat16]())
    print(DType.bfloat16 == DType.float16, DType.uint128 == DType.uint128)
    print(hash(DType.uint256) == hash(DType.uint256))
    print(DType.int128.is_signed(), DType.uint256.is_unsigned())
    print(DType._uint4.is_integral(), DType.bfloat16.is_floating_point())
    print(DType.float8_e5m2.is_float8(), DType.bfloat16.is_float8())
    print(DType.bfloat16.is_half_float(), DType.float8_e3m4.is_half_float())
    print(DType.float4_e2m1fn.is_numeric(), DType.float8_e5m2.is_numeric())
    print(DType.float8_e8m0fnu.is_numeric(), DType.int256.is_numeric())
    comptime folded = DType.float6_e3m2fn.is_numeric()
    print(folded)
    print(DType._from_str("bfloat16").value(), DType._from_str("int256").value())
    print(DType.mantissa_width[DType.bfloat16]())
    print(DType.exponent_bias[DType.float8_e5m2]())
    print(DType.mantissa_width[DType.float6_e3m2fn]())
    print(DType.max_exponent[DType.bfloat16]())
    print(DType.exponent_width[DType.float8_e4m3fn]())
    print(DType.exponent_bias[DType.float8_e4m3fnuz]())
