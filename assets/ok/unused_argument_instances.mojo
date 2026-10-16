# A runtime argument a body never uses leaves the generator a direct call
# names, as upstream's `RemoveUnusedParams` removes it first
# (`identifyUnusedArguments`) and then the parameters only its type named:
# `fa[n](a: Arr[n])` has one instance for `Arr[1]` and `Arr[2]`, and so do
# `fav`, whose owned `Arr` drops by running no code, `fmut`, and `fkw`,
# called by keyword. A non-generic `g` loses its argument too, and a
# function value of `g` keeps the full generator. An owned `NBox`, whose
# destructor prints, is used by its drop, so `fown` keeps one instance per
# `n`, and so is an owned `String`. A call passing a removed argument by
# position or by keyword drops it, and one omitting it passes nothing, so
# `fside()` never evaluates its default, which prints.


@fieldwise_init
struct Arr[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]


struct NBox[n: Int](Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", Self.n, self.v)


@no_inline
def fa[n: Int](a: Arr[n]) -> Int:
    return 3


@no_inline
def fav[n: Int](var a: Arr[n]) -> Int:
    return 4


@no_inline
def fmut[n: Int](mut a: Arr[n]) -> Int:
    return 5


@no_inline
def fkw[n: Int](k: Int, a: Arr[n]) -> Int:
    return k


@no_inline
def fbor[n: Int](b: NBox[n]) -> Int:
    return 6


@no_inline
def fown[n: Int](var b: NBox[n]) -> Int:
    return 7


@no_inline
def fstr(var s: String) -> Int:
    return 8


@no_inline
def fdef(x: Int = 5, y: Int = 6) -> Int:
    return y


@no_inline
def side() -> Int:
    print("side")
    return 1


@no_inline
def fside(x: Int = side()) -> Int:
    return 10


@no_inline
def g(x: Int) -> Int:
    return 9


@no_inline
def h(x: Int) -> Int:
    return x


def main() raises:
    print(fa(Arr[1](SIMD[DType.int64, 1](1))), fa(Arr[2](SIMD[DType.int64, 2](1))))
    print(fav(Arr[1](SIMD[DType.int64, 1](1))), fav(Arr[2](SIMD[DType.int64, 2](1))))
    var a1 = Arr[1](SIMD[DType.int64, 1](1))
    var a2 = Arr[2](SIMD[DType.int64, 2](1))
    print(fmut(a1), fmut(a2))
    print(
        fkw(a=Arr[1](SIMD[DType.int64, 1](1)), k=1),
        fkw(k=2, a=Arr[2](SIMD[DType.int64, 2](1))),
    )
    print(fbor(NBox[1](1)), fbor(NBox[2](2)))
    print(fown(NBox[1](3)), fown(NBox[2](4)))
    print(fstr(String("s")))
    print(fdef(), fdef(1), fdef(1, 2), fdef(y=3), fdef(x=4))
    print(fside())
    var f = g
    if Int(String("1")) > 5:
        f = h
    print(f(1), g(2))
