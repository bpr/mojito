# expect: function instantiation of `main` failed: constraint failed: Only floating point dtypes support +inf.
# `std.utils.numerics.inf` over an integral dtype fails its `comptime
# assert`, as at the pin.
from std.utils.numerics import inf


def main():
    print(inf[DType.int32]())
