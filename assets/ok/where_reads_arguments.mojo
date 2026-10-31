# A `where` clause reads the declaration's arguments, which the signature
# declares before it: `base.dtype` is the argument type's `dtype`
# parameter, an explicit binder or one a bare `Scalar` / `SIMD` infers, and
# a method's clause reads `self` and its other arguments alike. A trait
# requirement's clause reads its arguments too, and a witness declaring
# the same clause satisfies it.
def pinned[dtype: DType](base: Scalar[dtype]) -> Int where base.dtype == DType.float32:
    return 1


def squared(base: Scalar, exponent: Scalar) -> type_of(base) where base.dtype.is_floating_point():
    return base * base


def doubled(x: SIMD) -> type_of(x) where x.dtype.is_floating_point():
    return x + x


def matched(a: Scalar, b: Scalar) -> Int where a.dtype == b.dtype:
    return 3


def flagged(x: SIMD) -> Int where (x.dtype.is_floating_point(), "needs a float"):
    return 4


def integral[d: DType]() -> Int where d.is_integral():
    return 5


trait Offsets:
    def offset(self, x: SIMD) -> Int where x.dtype.is_floating_point():
        ...


struct Lanes[dtype: DType](Offsets):
    var v: Int

    def __init__(out self):
        self.v = 10

    def whole(self) -> Int where self.dtype.is_integral():
        return integral[Self.dtype]()

    def offset(self, x: SIMD) -> Int where x.dtype.is_floating_point():
        return self.v + 1


def offset_of[T: Offsets](t: T) -> Int:
    return t.offset(Float16(2))


def main():
    print(pinned(Float32(1.5)), squared(Float64(3), Float64(0.5)), doubled(SIMD[DType.float32, 2](1, 2)))
    print(matched(Int8(1), Int8(2)), flagged(Float16(1)))
    comptime k = doubled(Float32(1.25))
    print(k, Lanes[DType.int32]().whole(), Lanes[DType.uint8]().offset(SIMD[DType.float64, 4](0)))
    print(offset_of(Lanes[DType.int8]()))
