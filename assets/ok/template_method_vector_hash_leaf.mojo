# A generic struct's method hashing its field through the bound
# (`self.value.__hash__(hasher)`) derives, for a sized float and a sized
# integer instance, the hashed leaf each instance's own clone check accepts,
# and a direct `__hash__` on a vector or a sized scalar feeds the hasher as
# `hash()` does (`-0.0` folded first). The multi-lane vector instance is
# `conformance/probes/native_simd_instance_mangle.mojo`.
from std.hashlib import Hasher
from std.hashlib._ahash import AHasher


struct Box[T: Copyable & Deinitable & Hashable]:
    var value: Self.T

    def __init__(out self, var value: Self.T):
        self.value = value^

    def digest[H: Hasher](self, mut hasher: H):
        self.value.__hash__(hasher)


def main():
    var hasher = AHasher[SIMD[DType.uint64, 4](0)]()
    Box[Float32](Float32(1.5)).digest(hasher)
    Box[UInt8](UInt8(3)).digest(hasher)
    print(hasher^.finish())

    var negative = SIMD[DType.float32, 2](1.0, -0.0)
    var positive = SIMD[DType.float32, 2](1.0, 0.0)
    var first = AHasher[SIMD[DType.uint64, 4](0)]()
    negative.__hash__(first)
    UInt8(3).__hash__(first)
    var second = AHasher[SIMD[DType.uint64, 4](0)]()
    positive.__hash__(second)
    UInt8(3).__hash__(second)
    print(first^.finish() == second^.finish(), hash(negative) == hash(positive))
