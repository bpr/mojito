# expect: no constructor overload matches
# A `String` is no `Scalar[DType.int32]` element, whatever the width.
def f[n: SIMDLength](x: String) -> SIMD[DType.int32, n]:
    return SIMD[DType.int32, n](x, x)


def main():
    print(f[2]("a"))
