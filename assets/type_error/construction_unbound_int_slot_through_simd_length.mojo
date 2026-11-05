# expect: cannot infer type parameter 'n' of 'W'
# A `_` slot of type `Int` reached through `SIMD[other, Self.n]` is not
# bound: the pattern converts `n` to `SIMD`'s `SIMDLength` length, which
# no argument inverts, as the pin's "depends on an unresolved parameter".
struct W[dt: DType, n: Int]:
    def __init__[other: DType, //](out self, x: SIMD[other, Self.n], /):
        pass


def main():
    var v = SIMD[DType.float64, 4](1.0, 2.0, 3.0, 4.0)
    var w = W[DType.int32, _](v)
    print(w.n)
