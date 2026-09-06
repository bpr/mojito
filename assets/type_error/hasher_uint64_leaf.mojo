# expect: retired Mojito spelling
# `Hasher._update_with_simd` takes upstream's `SIMD[_, _]` vector; the
# `UInt64` leaf Mojito once accepted is a conformance error with a hint.
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

def main():
    print(hash[SumHasher](Int(1)))
