# `hash` of a vector whose dtype or width is a binder, inside a lane-keyed
# `def` or a method of a `DType`-keyed struct, served by one template: the
# elaborator instantiates the hasher's `_update_with_simd(mut self, value:
# SIMD[_, _])` at each vector type it reaches, at every width.
from std.hashlib.hasher import Fnv1a


def hs[dt: DType](v: Scalar[dt]) -> UInt64:
    return hash(v)


def h[dt: DType, n: Int](v: SIMD[dt, n]) -> UInt64:
    return hash(v)


def hw[dt: DType](v: SIMD[dt, 4]) -> UInt64:
    return hash(v)


def h1[dt: DType](v: SIMD[dt, 4]) -> UInt64:
    return hash(v[0])


def fnv[dt: DType, n: Int](v: SIMD[dt, n]) -> UInt64:
    var hasher = Fnv1a()
    hasher._update_with_simd(v)
    v.__hash__(hasher)
    return hasher^.finish()


struct Box[dt: DType]:
    var v: SIMD[Self.dt, 4]

    def __init__(out self, v: SIMD[Self.dt, 4]):
        self.v = v

    def h(self) -> UInt64:
        return hash(self.v)


def main():
    print(hs(Int64(7)), hs(Float32(1.5)))
    print(h[DType.int32, 4](SIMD[DType.int32, 4](1, 2, 3, 4)))
    print(h[DType.float32, 2](SIMD[DType.float32, 2](1.5, 2.5)))
    print(hw(SIMD[DType.uint8, 4](1, 2, 3, 4)))
    print(h1(SIMD[DType.int32, 4](1, 2, 3, 4)))
    print(Box[DType.uint16](SIMD[DType.uint16, 4](1, 2, 3, 4)).h())
    print(fnv[DType.int16, 8](SIMD[DType.int16, 8](1, 2, 3, 4, 5, 6, 7, 8)))
