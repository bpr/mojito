# expect: violated constraint
# `exp`'s SIMD overload requires a floating-point dtype, so an `int32`
# vector is rejected, as at the pin. The pin also declares a `ComplexSIMD`
# overload and so reports no matching function; without `ComplexSIMD`,
# the bundled `exp` is the sole candidate and reports its constraint.
from std.math import exp


def main():
    print(exp(SIMD[DType.int32, 2](1, 2)))
