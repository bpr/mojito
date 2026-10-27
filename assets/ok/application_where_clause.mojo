# A callable with a `where` clause is applied in a type or a parameter
# argument like any other: the clause binds the call's arguments and holds,
# or the enclosing declaration's own clause proves it.
def h[k: Int](x: Int) -> Int where k > 0:
    return x + k


struct G[k: Int]:
    @staticmethod
    def plus(n: Int) -> Int where Self.k > 2:
        return n + Self.k


def f[n: Int](x: SIMD[DType.int8, h[1](n)]) -> SIMD[DType.int8, h[1](n)]:
    return x


def g[n: Int](
    x: SIMD[DType.int8, G[3].plus(n)]
) -> SIMD[DType.int8, G[3].plus(n)]:
    return x


def e[m: Int]() where m > 0:
    var v = SIMD[DType.int8, h[m](1)](1)
    print(v)


def main():
    print(f[1](SIMD[DType.int8, h[1](1)](1)))
    print(g[1](SIMD[DType.int8, G[3].plus(1)](1)))
    e[1]()
    comptime c = h[2](3)
    print(c)
    comptime if h[1](1) == 2:
        print("two")
    var v = SIMD[DType.int8, h[1](1)](4)
    print(v)
    print(h[3](4))
