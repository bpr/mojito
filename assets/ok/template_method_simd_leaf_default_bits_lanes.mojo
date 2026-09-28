# A `Hasher`'s `_update_with_simd(mut self, value: SIMD[_, _])` holding
# `value.to_bits()` with its defaulted target in a local and casting each
# lane of it: the local's dtype is a parameter expression over the
# wildcard's hidden dtype, so `bits[i]` is a symbolic-lane scalar until the
# `cast[DType.uint64]()` closes it. Every per-call leaf clone, from a `Bool`
# lane to a demanded `SIMD[DType.int16, 2]`, folds both
# (`assets/ok/template_method_simd_leaf_default_bits.mojo` casts the whole
# vector first).
from std.hashlib import Hasher


struct LaneBitsHasher(Defaultable, Hasher):
    var acc: UInt64

    def __init__(out self):
        self.acc = UInt64(0xCBF29CE484222325)

    def _update_with_bytes(mut self, data: Span[Byte, _]):
        for i in range(len(data)):
            self.acc ^= data[i].cast[DType.uint64]()
            self.acc *= UInt64(0x100000001B3)

    def _update_with_simd(mut self, value: SIMD[_, _]):
        var bits = value.to_bits()
        var i = 0
        while i < bits.length:
            self.acc ^= bits[i].cast[DType.uint64]()
            self.acc *= UInt64(0x100000001B3)
            i += 1

    def update(mut self, value: Some[Hashable]):
        value.__hash__(self)

    def finish(var self) -> UInt64:
        return self.acc


@fieldwise_init
struct Pair(Hashable, Copyable, Movable):
    var lanes: SIMD[DType.int16, 2]

    def __hash__[H: Hasher](self, mut hasher: H):
        hasher._update_with_simd(self.lanes)


def main() raises:
    print(hash[LaneBitsHasher](Int(42)), hash[LaneBitsHasher](UInt8(7)))
    print(hash[LaneBitsHasher](Float32(1.5)), hash[LaneBitsHasher](True))
    print(hash[LaneBitsHasher](Pair(SIMD[DType.int16, 2](1, -1))))
    print(hash[LaneBitsHasher](String("ab")))
