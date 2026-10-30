# A compile-time call of a nested `def` that captures nothing takes a struct
# result as it takes a scalar one: a parameter argument (`first[mk()]()`), a
# method's nested `def` reading `Self` parameters, a generic or argument-taking
# one, one nested twice, and a local `comptime`.

from std.utils import StaticTuple


def first[n: Int, t: StaticTuple[SIMDLength, n]]() -> Int:
    var s = 0
    comptime for i in range(n):
        s += Int(t[i])
    return s


def outer[k: Int]():
    def mk() -> StaticTuple[SIMDLength, 8]:
        var r = StaticTuple[SIMDLength, 8](fill=SIMDLength(k))
        r[7] = 4
        return r

    def shifted(x: Int) -> StaticTuple[SIMDLength, 3]:
        return StaticTuple[SIMDLength, 3](fill=SIMDLength(x + k))

    def gen[m: Int]() -> StaticTuple[SIMDLength, m]:
        return StaticTuple[SIMDLength, m](fill=SIMDLength(k))

    def mid() -> Int:
        def deep() -> StaticTuple[SIMDLength, 2]:
            return StaticTuple[SIMDLength, 2](fill=SIMDLength(10 * k))

        return first[2, deep()]()

    print(first[8, mk()]())
    print(first[3, shifted(1)]())
    print(first[4, gen[4]()]())
    print(mid())
    comptime t = shifted(2)
    print(Int(t[0]))


struct W[length: SIMDLength]:
    @staticmethod
    def go() -> Int:
        def indices() -> StaticTuple[SIMDLength, 2 * Int(Self.length)]:
            var r = StaticTuple[SIMDLength, 2 * Int(Self.length)](
                fill=Self.length
            )
            r[0] = 4 * Self.length
            return r

        return first[2 * Int(Self.length), indices()]()


def main():
    outer[1]()
    outer[5]()
    print(W[4].go())
    print(W[2].go())
