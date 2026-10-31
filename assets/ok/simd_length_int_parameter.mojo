# A `SIMDLength` passed to an `Int` value parameter binds its lane count, in
# a body's compile-time application, in a parameter default, and in a call's
# bracket argument, positional or named.
from std.builtin.dtype import _uint_type_of_width


def f[src_width: SIMDLength](v: SIMD[DType.bool, src_width]) -> Int:
    comptime d = _uint_type_of_width[src_width]()
    print(d)
    return 0


def g[
    w: SIMDLength, //, dtype: DType = _uint_type_of_width[w]()
](v: SIMD[DType.bool, w]) -> Int:
    print(dtype)
    return 0


def mask[dtype: DType, width: Int]() -> SIMD[dtype, width]:
    return SIMD[dtype, width](width)


def lanes[width: Int]() -> Int:
    return width * 10


def h[dtype: DType, width: SIMDLength, //](
    x: SIMD[dtype, width]
) -> SIMD[dtype, width]:
    print(lanes[width=width]())
    return x + mask[dtype, width]()


def main():
    _ = f(SIMD[DType.bool, 16](fill=True))
    _ = g(SIMD[DType.bool, 16](fill=True))
    _ = g(SIMD[DType.bool, 8](fill=False))
    print(h(SIMD[DType.float32, 4](1, 2, 3, 4)))
    print(h(SIMD[DType.int64, 2](7, 8)))
