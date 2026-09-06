# Upstream's `Hasher` protocol spellings inside user `__hash__` bodies: both
# accepted signatures call `_update_with_simd` directly with a scalar, a
# `Float64`, a `Scalar[DType.bool]`, and a two-lane vector — each reaches the
# hasher as its own vector type (`SIMD[_, _]` infers per call), and the
# values are current Mojo's under both bundled hashers.
from std.hashlib import Hasher, default_comp_time_hasher

@fieldwise_init
struct Pair(Copyable, Hashable, Movable):
    var a: Int
    var b: Float64

    def __hash__[H: Hasher](self, mut hasher: H):
        hasher._update_with_simd(Int64(self.a))
        hasher._update_with_simd(self.b)
        hasher._update_with_simd(SIMD[DType.bool, 1](True))

@fieldwise_init
struct Tag(Copyable, Hashable, Movable):
    var n: Int8

    def __hash__(self, mut hasher: Some[Hasher]):
        hasher.update(self.n)
        hasher._update_with_simd(SIMD[DType.int32, 2](1, 2))

def main():
    print(hash(Pair(1, 1.5)), hash[default_comp_time_hasher](Pair(1, 1.5)))
    print(hash(Tag(-1)), hash[default_comp_time_hasher](Tag(-1)))
