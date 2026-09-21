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

def main():
    print(hash(Pair(1, String("hello"))))
    print(hash(Tag(5)))
