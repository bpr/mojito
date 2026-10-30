# A `SIMD` element list binds upstream's declared
# `__init__(out self, *elems: Scalar[Self.dtype], __list_literal__: NoneType = None)`:
# each element converts to the lane's scalar implicitly (a `SIMDLength`
# through its splat), extra elements past `length` are not read, and an
# initializer list into a `SIMD` slot takes the same overload.


def lanes[dt: DType, n: SIMDLength](x: Scalar[dt]) -> SIMD[dt, n]:
    return SIMD[dt, n](x, x)


def take(v: SIMD[DType.int32, 2]):
    print(v)


def main():
    var a = Int32(5)
    print(SIMD[DType.int32, 4](a, 2, a, 7))
    var m = SIMDLength(3)
    print(SIMD[DType.int, 2](m, m))
    print(SIMD[DType.int32, 2](1, 2, 3))
    print(SIMD[DType.int32, 2](1, 2, __list_literal__=None))
    print(SIMD[DType.bool, 4](True, False, True, True))
    print(SIMD[DType.float32, 4](1, 2.5, 3, 4))
    print(lanes[DType.int16, 2](Int16(9)))
    var v: SIMD[DType.int32, 2] = {1, 2}
    print(v)
    take({3, 4})
