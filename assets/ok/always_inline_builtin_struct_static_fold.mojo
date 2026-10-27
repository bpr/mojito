# A call of an `@always_inline("builtin")` static method in a parameter
# expression is its body's expression, its struct's parameters bound from
# the instance it is called on: `G[1].plus(n)` over `return n + Self.k` is
# `n + 1`, a call through `Self` folds too, and the fold drops a
# `comptime assert`, as the pinned Mojo folds them.


struct G[k: Int]:
    var x: SIMD[DType.int8, Self.plus(1)]

    def __init__(out self):
        self.x = SIMD[DType.int8, 1 + Self.k](2)

    @always_inline("builtin")
    @staticmethod
    def plus(n: Int) -> Int:
        return n + Self.k

    @always_inline("builtin")
    @staticmethod
    def plus2(n: Int) -> Int:
        return Self.plus(n) + Self.k

    @always_inline("builtin")
    @staticmethod
    def scaled[m: Int](n: Int) -> Int:
        return n * m + Self.k

    @always_inline("builtin")
    @staticmethod
    def get() -> Int:
        comptime assert Self.k > 0, "positive"
        return Self.k


struct S:
    @always_inline("builtin")
    @staticmethod
    def twice(n: Int) -> Int:
        return n + n

    @always_inline("builtin")
    @staticmethod
    def quad(n: Int) -> Int:
        return Self.twice(Self.twice(n))


def plus_one[n: Int](x: SIMD[DType.int8, G[1].plus(n)]) -> SIMD[DType.int8, n + 1]:
    return x

def plus_two[n: Int](x: SIMD[DType.int8, G[1].plus2(n)]) -> SIMD[DType.int8, n + 2]:
    return x

def scaled[n: Int](x: SIMD[DType.int8, G[2].scaled[3](n)]) -> SIMD[DType.int8, 3 * n + 2]:
    return x

def own[n: Int](x: SIMD[DType.int8, G[n].get()]) -> SIMD[DType.int8, n]:
    return x

def quad[n: Int](x: SIMD[DType.int8, S.quad(n)]) -> SIMD[DType.int8, 4 * n]:
    return x

def pair_len[n: Int](x: SIMD[DType.int8, Tuple[Int, Bool].__len__() * n]) -> SIMD[DType.int8, 2 * n]:
    return x

def main():
    print(plus_one[1](SIMD[DType.int8, 2](1)))
    print(plus_two[2](SIMD[DType.int8, 4](1)))
    print(scaled[2](SIMD[DType.int8, 8](1)))
    print(own[2](SIMD[DType.int8, 2](1)))
    print(quad[1](SIMD[DType.int8, 4](1)))
    print(pair_len[1](SIMD[DType.int8, 2](1)))
    print(G[3]().x)
    comptime c = G[-1].plus(1)
    print(c)
