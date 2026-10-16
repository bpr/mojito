# A runtime argument a body only passes on to a callee's removed argument
# is unused too, as upstream's `RemoveUnusedParams` walks callees first and
# rewrites each call to the clone without the argument: `fwd[n](a: Arr[n])`
# returning `fa(a) + 1` is the pin's `fwd…_REMOVED_ARG()`, one instance for
# `Arr[1]` and `Arr[2]`, and so are `fkw`, which passes it by keyword,
# `fchain`, which passes it to `fkw`, `fmove`, which moves it into an owned
# argument, `frecv`, whose callee removed its receiver, and `fwdi`, whose
# `x` an inlined `gi` drops. `fcopy` copies the struct into an owned
# argument, a copy of its own, and `fboth` reads it besides, so each keeps
# one instance per `n`; `fwdk` removes `a` but keeps `n`, which its callee
# reads.


@fieldwise_init
struct Arr[n: Int](ImplicitlyCopyable, Movable):
    var v: SIMD[DType.int64, Self.n]


@fieldwise_init
struct Grid[n: Int](Copyable, Movable):
    var k: Int

    @no_inline
    def k3(self) -> Int:
        return 3


@no_inline
def fa[n: Int](a: Arr[n]) -> Int:
    return 3


@no_inline
def fav[n: Int](var a: Arr[n]) -> Int:
    return 5


@no_inline
def fkeep[n: Int](a: Arr[n]) -> Int:
    return n


@no_inline
def fwd[n: Int](a: Arr[n]) -> Int:
    return fa(a) + 1


@no_inline
def fkw[n: Int](a: Arr[n]) -> Int:
    return fa(a=a) + 3


@no_inline
def fchain[n: Int](a: Arr[n]) -> Int:
    return fkw(a) + 4


@no_inline
def fmove[n: Int](var a: Arr[n]) -> Int:
    return fav(a^) + 2


@no_inline
def fcopy[n: Int](a: Arr[n]) -> Int:
    return fav(a) + 1


@no_inline
def fboth[n: Int](a: Arr[n]) -> Int:
    return fa(a) + Int(a.v[0])


@no_inline
def fwdk[n: Int](a: Arr[n]) -> Int:
    return fkeep(a) + 1


@no_inline
def frecv[n: Int](g: Grid[n]) -> Int:
    return g.k3() + 1


@always_inline("nodebug")
def gi(x: Int, y: Int) -> Int:
    return y


@no_inline
def fwdi(x: Int, y: Int) -> Int:
    return gi(x, y) + 1


def main():
    var a1 = Arr[1](SIMD[DType.int64, 1](1))
    var a2 = Arr[2](SIMD[DType.int64, 2](2))
    print(fwd(a1), fwd(a2))
    print(fkw(a1), fkw(a2))
    print(fchain(a1), fchain(a2))
    print(fmove(a1), fmove(a2))
    print(fcopy(a1), fcopy(a2))
    print(fboth(a1), fboth(a2))
    print(fwdk(a1), fwdk(a2))
    print(frecv(Grid[1](1)), frecv(Grid[2](2)))
    print(fwdi(1, 2), fwdi(3, 4))
