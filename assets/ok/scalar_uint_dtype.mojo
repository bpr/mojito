# `UInt` is upstream's `Scalar[DType.uint]`, as `Int` and `Float64` are the
# width-one `int` and `float64` vectors: a `Scalar[dt]` binder takes it, an
# annotation spells it either way, and `DType.uint` is an unsigned integral
# dtype of its own.
def twice[dt: DType](a: Scalar[dt]) -> Scalar[dt]:
    return a + a


def main():
    print(UInt.dtype)
    print(twice(UInt(3)), twice(Int(3)), twice(Float64(1.5)))
    var x: Scalar[DType.uint] = UInt(3)
    print(x, x == UInt(3))
    print(DType.uint, DType.uint.is_unsigned(), DType.uint.is_integral())
    print(DType.uint.is_signed(), DType.uint == DType.uint64)
    print(repr(UInt(5)), repr(Scalar[DType.uint](7)))
    var v = SIMD[DType.uint, 4](1, 2, 3, 4) + 1
    print(v, v[2], v.reduce_add())
