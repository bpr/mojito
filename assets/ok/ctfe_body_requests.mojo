# A compile-time evaluation in a function body is a request the elaborator
# serves from its worklist: a local `comptime` binding, a `comptime if`
# condition, a `comptime for` bound, or a `comptime(...)` operand that
# applies a callable runs once, at compile time, whatever its result type.


@fieldwise_init
struct Pair(Copyable, Writable):
    var a: Int
    var b: Int

    @staticmethod
    def scaled(n: Int) -> Int:
        return n * 10


def width(n: Int) -> Int:
    return n * 2


def label(n: Int) -> String:
    return String(t"n={n}")


def pair_of(n: Int) -> Pair:
    return Pair(n, n + 1)


def halves(n: Int) -> Tuple[Int, Int]:
    return (n // 2, n - n // 2)


def lanes[k: Int]() -> Int:
    comptime w = width(k)
    var v = SIMD[DType.int32, w](1)
    return Int(v.reduce_add())


def tagged[k: Int]() -> String:
    comptime s = label(k)
    return s


def main():
    comptime w = width(2)
    var v = SIMD[DType.int32, w](7)
    print(v, lanes[1](), lanes[2]())
    comptime s = label(3)
    comptime p = pair_of(4)
    comptime t = halves(7)
    comptime m = Pair.scaled(2) + width(1)
    print(s, tagged[5](), p.a, p.b, t[0], t[1], m)
    comptime if width(2) == 4:
        print("four")
    else:
        print("other")
    comptime for i in range(width(1)):
        print(i)
    print(comptime(width(5)))
