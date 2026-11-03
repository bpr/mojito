# expect: the SIMD type must be floating point
# `a + 1.5` over `a: Scalar[dt]` converts `1.5` through upstream's
# `@implicit SIMD.__init__(FloatLiteral)`; its instance at `int32` fails
# the float-dtype assert.
def h[dt: DType](a: Scalar[dt]):
    print(a + 1.5)


def main():
    h(Int32(2))
