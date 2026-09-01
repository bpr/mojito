# expect: does not conform to trait 'Hasher'
# `Hasher.finish` consumes the hasher and returns `UInt64`; a word-sized
# `UInt` result does not satisfy the protocol.
from std.hashlib import Hasher

struct NarrowHasher(Hasher):
    var state: UInt64

    def __init__(out self):
        self.state = UInt64(0)

    def _update_with_bytes(mut self, data: Span[Byte, _]):
        pass

    def _update_with_simd(mut self, value: UInt64):
        self.state += value

    def update(mut self, value: Some[Hashable]):
        value.__hash__(self)

    def finish(var self) -> UInt:
        return UInt(0)

def main():
    print(hash[NarrowHasher](Int(1)))
