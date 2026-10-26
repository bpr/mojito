# A protocol member or subscript accessor that never uses an argument leaves
# the generator its call names, as upstream's `RemoveUnusedParams` rewrites
# each call in library code: `E[n].__eq__` returning `True` reached by `==`
# and `!=`, `L[n].__len__` by `len`, `C[n].__iter__` by `for`,
# `C[n].__contains__` by `in`, `Arr[n].__getitem__(self, i: Int)` returning
# `i` by `a[i]`, and `S[n].__setitem__(mut self, i: Int, v: Int)` ignoring
# `v` by `s[i] = v`. The first five never read `self`, so each has one
# instance for `n = 1` and `n = 2`, and the subscript calls it with the
# index alone; the store passes no value, while `S`'s setter, which reads
# `self`, keeps one instance per `n`. `V[n].__getitem__(self, *idx: Int)`
# returning `self.k` passes no index at all.


struct E[n: Int](Copyable, Equatable, Movable):
    var v: SIMD[DType.int64, Self.n]

    def __init__(out self, x: Int):
        self.v = SIMD[DType.int64, Self.n](x)

    @no_inline
    def __eq__(self, other: Self) -> Bool:
        return True


struct L[n: Int](Copyable, Movable, Sized):
    var v: SIMD[DType.int64, Self.n]

    def __init__(out self, x: Int):
        self.v = SIMD[DType.int64, Self.n](x)

    @no_inline
    def __len__(self) -> Int:
        return 4


@fieldwise_init
struct RangeIter(Copyable, Iterator, Movable):
    comptime Element = Int
    var cur: Int
    var stop: Int

    def __next__(mut self) raises StopIteration -> Int:
        if self.cur >= self.stop:
            raise StopIteration()
        self.cur += 1
        return self.cur - 1


struct C[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]

    def __init__(out self, x: Int):
        self.v = SIMD[DType.int64, Self.n](x)

    @no_inline
    def __iter__(self) -> RangeIter:
        return RangeIter(0, 3)

    @no_inline
    def __contains__(self, x: Int) -> Bool:
        return True


struct Arr[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]

    def __init__(out self, x: Int):
        self.v = SIMD[DType.int64, Self.n](x)

    @no_inline
    def __getitem__(self, i: Int) -> Int:
        return i


struct S[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]
    var last: Int

    def __init__(out self, x: Int):
        self.v = SIMD[DType.int64, Self.n](x)
        self.last = 0

    def __getitem__(self, i: Int) -> Int:
        return self.last

    @no_inline
    def __setitem__(mut self, i: Int, v: Int):
        self.last = i


struct V[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]
    var k: Int

    def __init__(out self, x: Int):
        self.v = SIMD[DType.int64, Self.n](x)
        self.k = x

    @no_inline
    def __getitem__(self, *idx: Int) -> Int:
        return self.k


def main():
    print(E[1](1) == E[1](2), E[2](1) != E[2](2))
    print(len(L[1](1)), len(L[2](2)))
    var total = 0
    for i in C[1](1):
        total += i
    for i in C[2](2):
        total += i * 10
    print(total, 3 in C[1](1), 4 in C[2](2))
    var a = Arr[1](1)
    var b = Arr[2](2)
    print(a[3], b[4])
    var s = S[1](1)
    var r = S[2](2)
    s[5] = 7
    r[6] = 8
    print(s.last, r.last, s[0] + r[0])
    print(V[1](3)[1, 2], V[2](4)[3, 4])
