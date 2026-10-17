# A runtime argument a function only passes back to itself, at its own
# position by every self-recursive call, is unused, as upstream's
# `RemoveUnusedParams` reads through a self-recursive call: `frec[n](a: Arr[n],
# k: Int)` returning `frec(a, k - 1) + 1` is the pin's `frec…_REMOVED_ARG(i64)`,
# one instance for `Arr[1]` and `Arr[2]`, and so are `fkw`, which passes it
# back by keyword, `fdeep`, which passes it back from two calls, and
# `Grid[n].mrec`, whose `a` rides every self-call beside the receiver it reads.
# `fother` passes `a` and `b` back swapped, `fboth` reads `a` beside the
# forward, and `Grid.ping` and `Grid.pong` forward around a longer cycle, so
# each keeps one instance per `n`.


@fieldwise_init
struct Arr[n: Int](ImplicitlyCopyable, Movable):
    var v: SIMD[DType.int64, Self.n]


@fieldwise_init
struct Grid[n: Int](Copyable, Movable):
    var k: Int

    @no_inline
    def mrec(self, a: Arr[Self.n], d: Int) -> Int:
        if d == 0:
            return self.k
        return self.mrec(a, d - 1) + 1

    @no_inline
    def ping(self, a: Arr[Self.n], d: Int) -> Int:
        if d == 0:
            return 0
        return self.pong(a, d - 1) + 1

    @no_inline
    def pong(self, a: Arr[Self.n], d: Int) -> Int:
        if d == 0:
            return 0
        return self.ping(a, d - 1) + 1


@no_inline
def frec[n: Int](a: Arr[n], k: Int) -> Int:
    if k == 0:
        return 0
    return frec(a, k - 1) + 1


@no_inline
def fkw[n: Int](a: Arr[n], k: Int) -> Int:
    if k == 0:
        return 0
    return fkw(a=a, k=k - 1) + 2


@no_inline
def fdeep[n: Int](a: Arr[n], k: Int) -> Int:
    if k <= 1:
        return 1
    return fdeep(a, k - 1) + fdeep(a, k - 2)


@no_inline
def fother[n: Int](a: Arr[n], b: Arr[n], k: Int) -> Int:
    if k == 0:
        return 0
    return fother(b, a, k - 1) + 1


@no_inline
def fboth[n: Int](a: Arr[n], k: Int) -> Int:
    if k == 0:
        return Int(a.v[0])
    return fboth(a, k - 1) + 1


def main():
    var a1 = Arr[1](SIMD[DType.int64, 1](1))
    var a2 = Arr[2](SIMD[DType.int64, 2](2))
    print(frec(a1, 3), frec(a2, 4))
    print(fkw(a1, 3), fkw(a2, 4))
    print(fdeep(a1, 5), fdeep(a2, 6))
    print(fother(a1, a1, 3), fother(a2, a2, 4))
    print(fboth(a1, 3), fboth(a2, 4))
    print(Grid[1](1).mrec(a1, 3), Grid[2](2).mrec(a2, 4))
    print(Grid[1](1).ping(a1, 3), Grid[2](2).ping(a2, 4))
