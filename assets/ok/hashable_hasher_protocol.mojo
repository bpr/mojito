# `Hashable`'s requirement is `__hash__(self, mut hasher: Some[Hasher])`, also
# spelled `__hash__[H: Hasher](self, mut hasher: H)`; a conformer that omits it
# gets the reflective default, which feeds every field in declaration order —
# exactly what an explicit field-by-field implementation does.
from std.hashlib import Hasher

@fieldwise_init
struct Pair(Hashable, Copyable, Movable):
    var a: Int
    var b: String
    def __hash__[H: Hasher](self, mut hasher: H):
        self.a.__hash__(hasher)
        self.b.__hash__(hasher)

@fieldwise_init
struct Tag(Hashable, Copyable, Movable):
    var v: UInt8
    def __hash__(self, mut hasher: Some[Hasher]):
        hasher._update_with_simd(self.v)

@fieldwise_init
struct Explicit(Hashable, Copyable, Movable):
    var x: Int
    var y: Int
    def __hash__(self, mut hasher: Some[Hasher]):
        self.x.__hash__(hasher)
        self.y.__hash__(hasher)

@fieldwise_init
struct Reflective(Hashable, Copyable, Movable):
    var x: Int
    var y: Int

def main():
    print(hash(Pair(1, "hello")) == hash(Pair(1, "hello")))
    print(hash(Pair(1, "hello")) == hash(Pair(2, "hello")))
    print(hash(Tag(5)) == hash(Tag(5)), hash(Tag(5)) == hash(Tag(6)))
    print(hash(Explicit(1, 2)) == hash(Reflective(1, 2)))
    print(hash(Reflective(1, 2)) == hash(Reflective(2, 1)))
