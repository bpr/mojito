# expect: no overload matches
# `exp`'s SIMD overload requires a floating-point dtype, and an `int32`
# vector is no `_Expable`, so no overload matches, as at the pin.
from std.math import exp


def main():
    print(exp(SIMD[DType.int32, 2](1, 2)))
