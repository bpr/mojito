# `SIMD`'s `__hash__` is its declared library member and its `Hashable`
# conformance is its declaration: a floating `-0.0` lane hashes as `0.0`,
# folded through `to_bits`, `eq`, and `select`, for a direct call, `hash`,
# a reflective struct default, `Dict` and `Set` keys, a lane-keyed or
# `Hashable`-bound generic, and a compile-time `hash`.
from std.collections import Set
from std.hash import Hasher
from std.hash._fnv1a import Fnv1a


@fieldwise_init
struct P(Hashable, Copyable, Movable):
    var x: Int
    var y: Float64


def h_vec[dt: DType, w: SIMDLength](v: SIMD[dt, w]) -> UInt64:
    return hash[HasherType=Fnv1a](v)


def h_any[T: Hashable](v: T) -> UInt64:
    return hash[HasherType=Fnv1a](v)


def main() raises:
    print(hash(Int(5)) == hash(Int(5)), hash(Int(5)) == hash(Int(6)))
    print(hash(Float64(-0.0)) == hash(Float64(0.0)))
    print(
        hash(SIMD[DType.float32, 4](-0.0, 1, 2, 3))
        == hash(SIMD[DType.float32, 4](0.0, 1, 2, 3))
    )
    print(
        hash(UInt8(3)),
        hash(Int32(-7)),
        hash(Float32(-0.0)) == hash(Float32(0.0)),
    )
    print(hash(P(1, -0.0)) == hash(P(1, 0.0)))
    print(hash(True) == hash(True), hash(DType.int8) == hash(DType.int8))
    var h = Fnv1a()
    Int(3).__hash__(h)
    Float64(-0.0).__hash__(h)
    print(h^.finish())
    var d = Dict[Int, Int]()
    d[3] = 4
    print(d[3])
    comptime c = hash[HasherType=Fnv1a](Int(7))
    print(c == hash[HasherType=Fnv1a](Int(7)))
    print(
        h_vec(SIMD[DType.float16, 2](-0.0, 1))
        == h_vec(SIMD[DType.float16, 2](0.0, 1))
    )
    print(h_vec(SIMD[DType.int8, 4](1, 2, 3, 4)))
    print(h_any(Float32(-0.0)) == h_any(Float32(0.0)), h_any(UInt(9)))
    print(
        hash[HasherType=Fnv1a](Float64(-0.0)),
        hash[HasherType=Fnv1a](Float64(0.0)),
    )
    print(
        hash[HasherType=Fnv1a](Float64(1.5)),
        hash[HasherType=Fnv1a](Int64(-3)),
    )
    print(hash[HasherType=Fnv1a](SIMD[DType.bool, 4](True, False, True, False)))
    var s = Set[Float64]()
    s.add(-0.0)
    s.add(0.0)
    print(len(s))
    comptime k = hash[HasherType=Fnv1a](Float64(-0.0))
    print(k)
    comptime k2 = hash[HasherType=Fnv1a](SIMD[DType.float32, 2](-0.0, 2))
    print(k2 == hash[HasherType=Fnv1a](SIMD[DType.float32, 2](0.0, 2)))
