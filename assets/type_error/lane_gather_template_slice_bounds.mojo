# expect: constraint failed: output width must be a positive integer less than simd size
# A template-served slice must lie within the instance's receiver lanes; the
# instance it overruns fails, as at the pin, in `SIMD.slice`'s own
# `comptime assert`, the innermost failing instance.
def upper[dt: DType, w: Int](v: SIMD[dt, w]) -> SIMD[dt, 4]:
    return v.slice[4, offset=2]()


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(upper[DType.int32, 4](v))
