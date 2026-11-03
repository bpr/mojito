# expect: the SIMD type must be floating point
# `return 2.5` from `-> Scalar[dt]` converts through upstream's `@implicit
# SIMD.__init__(FloatLiteral)`; its instance at `int16` fails the
# float-dtype assert.
def k[dt: DType]() -> Scalar[dt]:
    return 2.5


def main():
    print(k[DType.int16]())
