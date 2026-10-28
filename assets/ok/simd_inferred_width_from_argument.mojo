# An unbound `_` width is bound by a one-argument construction: upstream's
# cast overload `__init__[other_dtype](value: SIMD[other_dtype, Self.length])`
# takes the width of the argument, which a literal or a scalar gives as one.
# Several elements never bind it (`assets/type_error/simd_inferred_width_from_elements.mojo`).
def main():
    var n = 3
    var f = 2.5
    var x32 = Int32(3)
    var u = UInt(3)
    print(SIMD[DType.uint8, _](300))
    print(SIMD[DType.float32, _](2.5))
    print(SIMD[DType.float64, _](5))
    print(SIMD[DType.uint8, _](n))
    print(SIMD[DType.uint8, _](x32))
    print(SIMD[DType.float32, _](f))
    print(SIMD[DType.uint, _](u))
    print(SIMD[DType.float32, _](n))
    var v: SIMD[DType.int, _] = SIMD[DType.int, _](7)
    print(len(v), v)
