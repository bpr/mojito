# expect: the SIMD type must be floating point
# `std.math`'s `min(x: SIMD[dtype, _], y: type_of(x))` binds `Int` from `3`,
# and `2.5` converts through upstream's `@implicit
# SIMD.__init__(FloatLiteral)`, whose float-dtype assert fails.
def main():
    print(min(3, 2.5))
