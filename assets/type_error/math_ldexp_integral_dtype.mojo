# expect: violated constraint
# `ldexp` is constrained to a floating-point dtype.
from std.math import ldexp


def main():
    print(ldexp(SIMD[DType.int32, 2](1, 2), SIMD[DType.int32, 2](1, 1)))
