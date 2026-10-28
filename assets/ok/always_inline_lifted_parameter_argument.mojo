# An inlined callee's `comptime assert` over a binder bound to an
# application, or to a local `comptime` value, is judged on the
# application's value, as at the pin.
from std.builtin.dtype import _uint_type_of_width
from std.sys.info import bit_width_of


@always_inline
def k[width: Int]() -> Int:
    comptime assert width in (8, 16, 32, 64), "bad width"
    return width + 1


def unsigned_of[dt: DType]() -> DType:
    comptime w = bit_width_of[dt]()
    return _uint_type_of_width[w]()


def f[dt: DType]() -> Int:
    comptime w = bit_width_of[dt]()
    return k[w]() + k[bit_width_of[dt]() * 2]()


def main():
    print(_uint_type_of_width[bit_width_of[DType.int16]()]())
    print(unsigned_of[DType.float32]())
    print(k[bit_width_of[DType.int32]()]())
    print(f[DType.int16]())
