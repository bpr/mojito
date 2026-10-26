# expect: declares conformance to trait 'Hasher' but is missing method 'finish'
# A Hasher needs `__init__`, `_update_with_bytes`, `_update_with_simd`,
# `update`, and `finish`; omitting `finish` is a conformance error.
from std.hash import Hasher

struct BrokenHasher(Hasher):
    var state: UInt64

    def __init__(out self):
        self.state = UInt64(0)

    def _update_with_bytes(mut self, data: Span[Byte, _]):
        pass

    def _update_with_simd(mut self, value: SIMD[_, _]):
        pass

    def update(mut self, value: Some[Hashable]):
        value.__hash__(self)

def main():
    print(hash[BrokenHasher](Int(1)))
