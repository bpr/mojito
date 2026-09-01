# A user `Hasher`: the compiler feeds every scalar leaf as one normalized
# `UInt64` (`_update_with_simd`), a string's bytes as a `Span[Byte, _]`
# (`_update_with_bytes`), and `update` dispatches a Hashable value's own
# `__hash__`. The conformer works through `hash[H]` and as a `Dict` hasher.
from std.hashlib import Hasher

struct SumHasher(Defaultable, Hasher):
    var total: UInt64

    def __init__(out self):
        self.total = UInt64(0)

    def _update_with_bytes(mut self, data: Span[Byte, _]):
        for i in range(len(data)):
            self.total += data[i].cast[DType.uint64]()

    def _update_with_simd(mut self, value: UInt64):
        self.total += value

    def update(mut self, value: Some[Hashable]):
        value.__hash__(self)

    def finish(var self) -> UInt64:
        return self.total

@fieldwise_init
struct Point(Hashable, Copyable, Movable):
    var x: Int
    var y: Int

def main() raises:
    print(hash[SumHasher](Int(40)))
    print(hash[SumHasher](Point(40, 2)))
    print(hash[SumHasher](String("ab")))
    var d = Dict[Int, Int, SumHasher]()
    d[1] = 10
    d[2] = 20
    print(d[1], d[2], len(d))
