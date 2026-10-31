# expect: the SIMD type must be floating point
# `SIMD[DType.int32, 2](2.5)` binds the declared `@implicit
# SIMD.__init__(FloatLiteral)`, whose float-dtype assert fails the instance.
def main():
    print(SIMD[DType.int32, 2](2.5))
