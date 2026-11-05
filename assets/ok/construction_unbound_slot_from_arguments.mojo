# A `_` in a construction's bracket leaves its slot to the selected
# initializer, which binds it from the arguments or from its refined
# `out self`: the cast overload binds `SIMD`'s width from a vector, the
# `Intable` one a scalar's, the `Bool` one a mask's dtype, and a call of
# the partial alias `Scalar` leaves `dtype` unbound the same way. A user
# struct's slot binds the same way, a `SIMDLength` one through `SIMD`'s
# own, and a type slot through the initializer's parameter.
from std.utils import StaticTuple


struct Lanes[dt: DType, n: SIMDLength]:
    def __init__[other: DType, //](out self, x: SIMD[other, Self.n], /):
        pass


struct Mask[dt: DType, n: Int]:
    def __init__(out self: Mask[DType.bool, Self.n], value: Bool, /):
        pass


struct Filled[T: ImplicitlyCopyable & Writable & Deinitable, n: Int]:
    var x: Self.T

    def __init__(out self, *, fill: Self.T):
        self.x = fill


def main():
    var v = SIMD[DType.float64, 4](1.0, 2.0, 3.0, 4.0)
    var w = SIMD[DType.float64, 2](1.5, 2.5)
    print(SIMD[DType.int32, _](v))
    print(SIMD[DType.float64, _](w))
    print(SIMD[DType.uint8, _](SIMD[DType.int, 2](300, 5)))
    print(SIMD[DType.int, _](True))
    print(SIMD[_, 1](True), Scalar(False), Scalar(Float32(1.5)))
    var lanes = Lanes[DType.int32, _](v)
    print(Int(lanes.n), lanes.dt)
    var mask = Mask[_, 1](True)
    print(mask.dt, mask.n)
    var filled = Filled[_, 3](fill=Int64(5))
    print(filled.x, filled.n)
    var tuple = StaticTuple[_, 3](fill=Int64(2))
    print(tuple[0], tuple[2])
