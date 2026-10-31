# expect: violated constraint
# `log` is constrained to a floating-point dtype.
from std.math import log


def main():
    print(log(SIMD[DType.int32, 2](1, 2)))
