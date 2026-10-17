# A dunder's runtime argument its body never uses leaves the generator an
# operator, a subscript, or a call names, as upstream's `RemoveUnusedParams`
# removes it first and then the parameters only its type named:
# `Grid[n].__add__(self, o: Arr[n])` returning `self.k + 1`, reached by
# `g + a` and by `g.__add__(a)`, `Grid[n].__getitem__(self, i: Int, a: Arr[n])`
# reached by `g[i, a]`, `Cell[n].__getitem__(self, a: Arr[n])`, reached by
# `c[a]`, and `Row[n].__getitem__(self, s: Slice)`, reached by `r[1:2]`,
# each have one instance for `n = 1` and `n = 2`, while `Grid.__sub__`,
# which reads its operand, keeps one per `n`.


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
    def __sub__(self, o: Arr[Self.n]) -> Int:
        return self.k - Int(o.v[0])

    @no_inline
    def __getitem__(self, i: Int, a: Arr[Self.n]) -> Int:
        return self.k + i


@fieldwise_init
struct Cell[n: Int](Copyable, Movable):
    var k: Int

    @no_inline
    def __getitem__(self, a: Arr[Self.n]) -> Int:
        return self.k * 2


@fieldwise_init
struct Row[n: Int](Copyable, Movable):
    var k: Int

    @no_inline
    def __getitem__(self, s: Slice) -> Int:
        return self.k * 3


def main():
    var a1 = Arr[1](SIMD[DType.int64, 1](4))
    var a2 = Arr[2](SIMD[DType.int64, 2](5))
    var g1 = Grid[1](10)
    var g2 = Grid[2](20)
    print(g1 + a1, g2 + a2)
    print(g1.__add__(a1), g2.__add__(a2))
    print(g1 - a1, g2 - a2)
    print(g1[3, a1], g2[4, a2])
    var c1 = Cell[1](7)
    var c2 = Cell[2](8)
    print(c1[a1], c2[a2])
    var r1 = Row[1](7)
    var r2 = Row[2](8)
    print(r1[1:2], r2[0:1])
