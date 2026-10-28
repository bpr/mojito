# A `SIMDLength` passed to an `Int` value parameter binds its lane count, in
# a body's compile-time application and in a parameter default.
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


def main():
    _ = f(SIMD[DType.bool, 16](fill=True))
    _ = g(SIMD[DType.bool, 16](fill=True))
    _ = g(SIMD[DType.bool, 8](fill=False))
