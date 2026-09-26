# A `Hasher`'s `_update_with_simd(mut self, value: SIMD[_, _])` is checked
# once, by source validation, with the wildcard parameter viewed as a
# lane-shaped vector, and every per-call leaf clone inherits its facts
# (`docs/notes/instantiation-from-template.md`, `SIMD_BINDERS` and
# `SIMD_INTRINSICS`): the reinterpretation `to_bits[DType.uint64]()`, a
# `while` over `.length`, lane reads folded into a `UInt64` field, a
# `reduce_add()` of the bits, and a width test selecting a lane-pair loop.
# Each clone bakes the binder to one vector type, from a `Bool` lane to a
# demanded `SIMD[DType.uint8, 4]`, and folds the dtype and width into what
# the template left open.
from std.hashlib import Hasher


struct FoldHasher(Defaultable, Hasher):
    var acc: UInt64

    def __init__(out self):
        self.acc = UInt64(0xCBF29CE484222325)

    def _update_with_bytes(mut self, data: Span[Byte, _]):
        for i in range(len(data)):
            self.acc ^= data[i].cast[DType.uint64]()
            self.acc *= UInt64(0x100000001B3)

    def _update_with_simd(mut self, value: SIMD[_, _]):
        var bits = value.to_bits[DType.uint64]()
        var i = 0
        while i < bits.length:
            self.acc ^= bits[i]
            self.acc *= UInt64(0x100000001B3)
            i += 1

    def update(mut self, value: Some[Hashable]):
        value.__hash__(self)

    def finish(var self) -> UInt64:
        return self.acc


struct PairHasher(Defaultable, Hasher):
    var acc: UInt64
    var lanes: Int

    def __init__(out self):
        self.acc = UInt64(17)
        self.lanes = 0

    def _update_with_bytes(mut self, data: Span[Byte, _]):
        for i in range(len(data)):
            self._mix(data[i].cast[DType.uint64]())

    def _update_with_simd(mut self, value: SIMD[_, _]):
        var bits = value.to_bits[DType.uint64]()
        self.lanes += bits.length
        if bits.length == 1:
            self._mix(bits[0])
        else:
            var i = 0
            while i < bits.length:
                self._pair(bits[i], bits[i + 1])
                i += 2
        self.acc ^= bits.reduce_add()

    def _mix(mut self, word: UInt64):
        self.acc = (self.acc ^ word) * UInt64(0x9E3779B97F4A7C15)

    def _pair(mut self, low: UInt64, high: UInt64):
        self.acc = (self.acc + low) * UInt64(31) + high

    def update(mut self, value: Some[Hashable]):
        value.__hash__(self)

    def finish(var self) -> UInt64:
        return self.acc ^ UInt64(self.lanes)


@fieldwise_init
struct Quad(Hashable, Copyable, Movable):
    var lanes: SIMD[DType.uint8, 4]

    def __hash__[H: Hasher](self, mut hasher: H):
        hasher._update_with_simd(self.lanes)


def main() raises:
    print(hash[FoldHasher](Int(42)), hash[PairHasher](Int(42)))
    print(hash[FoldHasher](Int64(-1)), hash[PairHasher](Int64(-1)))
    print(hash[FoldHasher](UInt8(7)), hash[PairHasher](UInt8(7)))
    print(hash[FoldHasher](Float32(1.5)), hash[PairHasher](Float32(1.5)))
    print(hash[FoldHasher](True) == hash[FoldHasher](True), hash[PairHasher](True) == hash[PairHasher](False))
    print(hash[FoldHasher](Quad(SIMD[DType.uint8, 4](1, 2, 3, 4))), hash[PairHasher](Quad(SIMD[DType.uint8, 4](1, 2, 3, 4))))
    print(hash[FoldHasher](String("ab")), hash[PairHasher](String("ab")))
    var d = Dict[Int, Int, PairHasher]()
    d[1] = 10
    d[2] = 20
    print(d[1], d[2], len(d))
