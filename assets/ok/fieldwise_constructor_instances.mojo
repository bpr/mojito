# A fieldwise construction calls the struct's synthesized `__init__`, which
# inlines like any `nodebug` constructor, so a generic function that builds
# a value of a struct over its parameter, and reads nothing else of the
# parameter, keeps one instance: `mkf` reads only the `Int` field of
# `Fw[n]`, and `plain_nested`'s nested `inner` drops the `Arr[n]` argument
# it never reads, taking the construction of that argument with it.


@fieldwise_init
struct Fw[n: Int]:
    var v: Int


def mkf[n: Int]() -> Int:
    return Fw[n](5).v


@fieldwise_init
struct Arr[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]


def plain_nested[n: Int](y: Int) -> Int:
    def inner(a: Arr[n], y: Int) -> Int:
        return y + 1

    return inner(Arr[n](SIMD[DType.int64, n](1)), y)


@fieldwise_init
struct Pt(ImplicitlyCopyable):
    var x: Int
    var y: Int


def main():
    print(mkf[1](), mkf[2]())
    print(plain_nested[1](1), plain_nested[2](2), plain_nested[4](3))
    var p = Pt(1, y=2)
    print(p.x, p.y)
