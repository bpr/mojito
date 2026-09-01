from std.hashlib import Hasher

@fieldwise_init
struct Pair(Hashable, Copyable, Movable):
    var a: Int
    var b: String

    def __hash__[H: Hasher](self, mut hasher: H):
        hasher.update(self.a)
        hasher.update(self.b)

@fieldwise_init
struct Tag(Hashable, Copyable, Movable):
    var v: UInt8

    def __hash__(self, mut hasher: Some[Hasher]):
        hasher.update(self.v)

def main():
    print(hash(Pair(1, String("hello"))))
    print(hash(Tag(5)))
