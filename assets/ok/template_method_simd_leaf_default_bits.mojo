# A `Hasher`'s `_update_with_simd(mut self, value: SIMD[_, _])` reading
# `value.to_bits()` with its defaulted target: source validation types the
# target as the unsigned dtype of the lane's width, a parameter expression
# over the wildcard's hidden dtype, and the `cast[DType.uint64]()` of it as a
# lane-shaped vector. Every per-call leaf clone, from a `Bool` lane to a
# demanded `SIMD[DType.int16, 2]`, folds both and inherits the template's
# facts (`assets/ok/template_method_simd_leaf.mojo` spells the target).
from std.hashlib import Hasher


struct BitsHasher(Defaultable, Hasher):
    var acc: UInt64

    def __init__(out self):
        self.acc = UInt64(0xCBF29CE484222325)

    def _update_with_bytes(mut self, data: Span[Byte, _]):
        for i in range(len(data)):
            self.acc ^= data[i].cast[DType.uint64]()
            self.acc *= UInt64(0x100000001B3)

    def _update_with_simd(mut self, value: SIMD[_, _]):
        var bits = value.to_bits().cast[DType.uint64]()
        var i = 0
        while i < bits.length:
            self.acc ^= bits[i]
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
    print(hash[BitsHasher](Int(42)), hash[BitsHasher](UInt8(7)))
    print(hash[BitsHasher](Float32(1.5)), hash[BitsHasher](True))
    print(hash[BitsHasher](Pair(SIMD[DType.int16, 2](1, -1))))
    print(hash[BitsHasher](String("ab")))
