# An inlined callee's `comptime assert` over a binder bound to an
# application fails on the application's value, as at the pin.
# expect: constraint failed: bad width
from std.sys.info import bit_width_of


@always_inline
def k[width: Int]() -> Int:
    comptime assert width in (8, 16, 32, 64), "bad width"
    return width + 1


def main():
    print(k[bit_width_of[DType.int8]() * 3]())
