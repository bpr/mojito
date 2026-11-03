# expect: the SIMD type must be floating point
# Each element of the `*elems: Scalar[Self.dtype]` list converts through
# upstream's `@implicit SIMD.__init__(FloatLiteral)`, whose float-dtype
# assert fails at `int32`.
def main():
    var v = SIMD[DType.int32, 2](1.5, 2.5)
    print(v)
