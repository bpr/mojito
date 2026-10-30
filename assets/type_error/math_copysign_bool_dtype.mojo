# expect: function instantiation of `main` failed: constraint failed: operands must be a numeric type
# `std.math.copysign` over `bool` lanes fails its `comptime assert`, as at
# the pin.
from std.math import copysign


def main():
    print(copysign(Scalar[DType.bool](True), Scalar[DType.bool](False)))
