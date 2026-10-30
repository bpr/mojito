# expect: function instantiation of `main` failed: constraint failed: Only floating point dtypes support NaN.
# `std.utils.numerics.nan` over an integral dtype fails its `comptime
# assert`, as at the pin.
from std.utils.numerics import nan


def main():
    print(nan[DType.int32]())
