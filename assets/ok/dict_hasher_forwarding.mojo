# A Dict's hasher parameter reaches every hashing site on both backends: the
# insert's single hash (shared by its bucket probe and the cached entry hash,
# as upstream's `_insert`) and the lookup's probe.
# The hasher reports each leaf it receives, so a fallback to the default
# hasher anywhere would change the output (or miss the key).
from std.hashlib import Hasher

struct LoudHasher(Defaultable, Hasher):
    var total: UInt64

    def __init__(out self):
        self.total = UInt64(0)

    def _update_with_bytes(mut self, data: Span[Byte, _]):
        print("bytes", len(data))
        for i in range(len(data)):
            self.total += data[i].cast[DType.uint64]()

    def _update_with_simd(mut self, value: SIMD[_, _]):
        print("simd", value.to_bits[DType.uint64]())
        self.total += value.to_bits[DType.uint64]().reduce_add()

    def update(mut self, value: Some[Hashable]):
        value.__hash__(self)

    def finish(var self) -> UInt64:
        return self.total

def main() raises:
    var d = Dict[Int, Int, LoudHasher]()
    d[7] = 1
    print(d[7])
