# expect: the SIMD type must be floating point
# `Int8.__add__(rhs: Self)` converts `2.5` through upstream's `@implicit
# SIMD.__init__(FloatLiteral)`, whose float-dtype assert fails at `Int8`.
def main():
    print(Int8(3) + 2.5)
