# An argument a body passes only to a dunder's removed argument is unused
# in turn, as upstream's `RemoveUnusedParams` rewrites the dunder's call
# first: `fwd_add[n]` returning `g + a`, `fwd_get[n]` returning `g[a]`,
# `fwd_radd[n]` returning `a + g` (`g.__radd__(a)`), and `fwd_multi[n]`
# returning `g[i, a]`, each lose `a` and with it `n`, so each has one
# instance for `n = 1` and `n = 2`, while `fwd_mul[n]`, whose `__mul__`
# ignores its receiver, loses `g` and keeps `a`, and `fwd_sub[n]`, whose
# `__sub__` reads its operand, keeps both, with one instance per `n`.
# `a1 + g1` itself calls the `__radd__` generator without its operand,
# which names no `Grid` instance. `Ring[n].__add__` and `__getitem__`,
# which pass their operand only back to themselves, through `+` and a
# subscript, lose it and `n` too.


@fieldwise_init
struct Arr[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]


@fieldwise_init
struct Grid[n: Int](Copyable, Movable):
    var k: Int

    @no_inline
    def __add__(self, o: Arr[Self.n]) -> Int:
        return self.k + 1

    @no_inline
    def __radd__(self, o: Arr[Self.n]) -> Int:
        return self.k + 2

    @no_inline
    def __sub__(self, o: Arr[Self.n]) -> Int:
        return self.k - Int(o.v[0])

    @no_inline
    def __mul__(self, o: Arr[Self.n]) -> Int:
        return Int(o.v[0]) * 3

    @no_inline
    def __getitem__(self, a: Arr[Self.n]) -> Int:
        return self.k * 2


@fieldwise_init
struct Pair[n: Int](Copyable, Movable):
    var k: Int

    @no_inline
    def __getitem__(self, i: Int, a: Arr[Self.n]) -> Int:
        return self.k + i


@fieldwise_init
struct Ring[n: Int](Copyable, Movable):
    var k: Int

    @no_inline
    def __add__(self, o: Arr[Self.n]) -> Int:
        if self.k > 0:
            return Ring[Self.n](self.k - 1) + o
        return 7

    @no_inline
    def __getitem__(self, o: Arr[Self.n]) -> Int:
        if self.k > 0:
            return Ring[Self.n](self.k - 1)[o]
        return 9


@no_inline
def fwd_add[n: Int](g: Grid[n], a: Arr[n]) -> Int:
    return g + a


@no_inline
def fwd_radd[n: Int](g: Grid[n], a: Arr[n]) -> Int:
    return a + g


@no_inline
def fwd_get[n: Int](g: Grid[n], a: Arr[n]) -> Int:
    return g[a]


@no_inline
def fwd_multi[n: Int](p: Pair[n], i: Int, a: Arr[n]) -> Int:
    return p[i, a]


@no_inline
def fwd_mul[n: Int](g: Grid[n], a: Arr[n]) -> Int:
    return g * a


@no_inline
def fwd_sub[n: Int](g: Grid[n], a: Arr[n]) -> Int:
    return g - a


def main():
    var a1 = Arr[1](SIMD[DType.int64, 1](4))
    var a2 = Arr[2](SIMD[DType.int64, 2](5))
    var g1 = Grid[1](10)
    var g2 = Grid[2](20)
    var p1 = Pair[1](30)
    var p2 = Pair[2](40)
    print(fwd_add(g1, a1), fwd_add(g2, a2))
    print(fwd_radd(g1, a1), fwd_radd(g2, a2))
    print(a1 + g1, a2 + g2)
    print(fwd_get(g1, a1), fwd_get(g2, a2))
    print(fwd_multi(p1, 3, a1), fwd_multi(p2, 4, a2))
    print(fwd_mul(g1, a1), fwd_mul(g2, a2))
    print(fwd_sub(g1, a1), fwd_sub(g2, a2))
    print(Ring[1](3) + a1, Ring[2](2) + a2)
    print(Ring[1](3)[a1], Ring[2](2)[a2])
