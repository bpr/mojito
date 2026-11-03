# expect: ambiguous
# `2.5` converts to `Float32` and to `Int8` through the same declared
# `@implicit SIMD.__init__(FloatLiteral)`, one conversion each, so the
# call is ambiguous, as at the pin.
def g(x: Float32):
    print("f32", x)


def g(x: Int8):
    print("i8", x)


def main():
    g(2.5)
