# A nested `def` and a function with a borrowed `*args` lose an argument
# their body never uses, as upstream's `RemoveUnusedParams` treats them like
# any generator: `inner[n](a: Arr[n])` has one instance for `Arr[1]` and
# `Arr[2]`, `cap` loses `x` and keeps its capture `k`, and `c` loses the
# capture it never reads. `ign`, called directly and passed to `apply`,
# keeps its full generator for the function value beside the reduced one
# its call names. `fv[n](k, *args: Arr[n])` has one instance for every `n`,
# `fw` one for every arity, and `fpre` loses `a` but keeps the pack it
# reads. An owned pack's drop runs code, so `fo` keeps one instance per
# `n`. `fused` reads its pack, and has one instance for every arity, as
# every homogeneous collector does. A call of a nested `def` by name is a
# direct call of its lifted body, so `h` loses `x`, which only forwards to
# `ign2`'s removed argument, and has one instance.


@fieldwise_init
struct Arr[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]


@no_inline
def apply(f: def(Int) thin -> Int) -> Int:
    return f(4)


def outer(k: Int) -> Int:
    @no_inline
    def inner[n: Int](a: Arr[n]) -> Int:
        return 3

    @no_inline
    def cap(x: Int) {imm k} -> Int:
        return k + 1

    return (
        inner(Arr[1](SIMD[DType.int64, 1](1)))
        + inner(Arr[2](SIMD[DType.int64, 2](1)))
        + cap(5)
    )


def cap2(k: Int) -> Int:
    @no_inline
    def c(x: Int) {imm k} -> Int:
        return x

    return c(1)


def esc(k: Int) -> Int:
    @no_inline
    def ign(x: Int) -> Int:
        return 3

    return apply(ign) + ign(9)


@no_inline
def fv[n: Int](k: Int, *args: Arr[n]) -> Int:
    return k


@no_inline
def fw(k: Int, *args: Int) -> Int:
    return k


@no_inline
def fpre[n: Int](a: Arr[n], *args: Int) -> Int:
    return len(args)


@no_inline
def fo[n: Int](k: Int, var *args: Arr[n]) -> Int:
    return k


@no_inline
def fused(*args: Int) -> Int:
    return len(args)


@no_inline
def h[n: Int](x: Arr[n]) -> Int:
    @no_inline
    def ign2(y: Arr[n]) -> Int:
        return 3

    return ign2(x)


def main() raises:
    print(outer(2), cap2(2), esc(1))
    print(
        fv(1, Arr[1](SIMD[DType.int64, 1](1))),
        fv(2, Arr[2](SIMD[DType.int64, 2](1))),
    )
    print(fw(1, 2), fw(3, 4, 5))
    print(
        fpre(Arr[1](SIMD[DType.int64, 1](1)), 1, 2),
        fpre(Arr[2](SIMD[DType.int64, 2](1)), 1, 2, 3),
    )
    print(
        fo(1, Arr[1](SIMD[DType.int64, 1](1))),
        fo(2, Arr[2](SIMD[DType.int64, 2](1))),
    )
    print(fused(1, 2))
    print(h(Arr[1](SIMD[DType.int64, 1](1))) + h(Arr[2](SIMD[DType.int64, 2](1))))
