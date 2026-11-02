# expect: no overload matches
# `exp`'s SIMD overload requires a floating-point dtype, so an `int32`
# vector matches no overload, as at the pin.
from std.math import exp


def main():
    print(exp(SIMD[DType.int32, 2](1, 2)))
