# expect: type mismatch for return
# A field read of a struct construction in a type is the field of its
# `__init__`'s application, never folded by the check, as at the pin.
@fieldwise_init
struct Extent(ImplicitlyCopyable, Movable):
    var rows: Int
    var cols: Int


def f[n: Int](x: SIMD[DType.int8, Extent(n, 1).rows]) -> SIMD[DType.int8, n]:
    return x


def main():
    print(f[2](SIMD[DType.int8, 2](1)))
