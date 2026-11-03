# expect: the SIMD type must be floating point
# The argument converts through upstream's `@implicit
# SIMD.__init__(FloatLiteral)`, whose float-dtype assert fails at `Int32`.
def f(x: Int32):
    print(x)


def main():
    f(2.5)
