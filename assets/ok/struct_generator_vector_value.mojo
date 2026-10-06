# A struct keyed on a vector value is a generator: `Self.key` reads the
# instance's value in an initializer, a method, and a static method, and an
# alias names one instance as an application of the template does.
struct Keyed[key: SIMD[DType.uint64, 4]](Copyable, Movable):
    var state: UInt64

    def __init__(out self):
        self.state = Self.key[0] ^ Self.key[3]

    def mix(self, value: UInt64) -> UInt64:
        var k = Self.key
        return (self.state + value) ^ k[1] ^ k[2]

    @staticmethod
    def lanes() -> UInt64:
        return Self.key.reduce_add()


comptime ZeroKeyed = Keyed[SIMD[DType.uint64, 4](0)]


def main():
    var a = Keyed[SIMD[DType.uint64, 4](1, 2, 4, 8)]()
    print(a.state, a.mix(16))
    print(Keyed[SIMD[DType.uint64, 4](1, 2, 4, 8)].lanes())
    var z = ZeroKeyed()
    print(z.state, z.mix(5))
