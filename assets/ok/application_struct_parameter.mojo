# A compile-time call applies a function whose runtime parameter is a
# struct, as at the pin: `mylen(Foo(n))` in a signature and in a `comptime`
# binding, a register-passable struct taken by a builtin constructor, and a
# parameter typed by the callee's own type parameter at a struct argument.
@fieldwise_init
struct Foo(ImplicitlyCopyable):
    var n: Int


def mylen(f: Foo) -> Int:
    return f.n


def idt[T: ImplicitlyCopyable](x: T) -> T:
    return x


struct L(TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        self.v = v


struct M(TrivialRegisterPassable):
    var l: L
    var w: Int

    @always_inline("builtin")
    def __init__(out self, l: L, w: Int):
        self.l = l
        self.w = w


def lanes[n: Int]() -> SIMD[DType.int8, mylen(Foo(n))]:
    return SIMD[DType.int8, mylen(Foo(n))](1)


def same[n: Int]() -> SIMD[DType.int8, idt(Foo(n)).n]:
    return SIMD[DType.int8, idt(Foo(n)).n](3)


def doubled[
    n: Int
](x: SIMD[DType.int8, M(L(n), 3).l.v * 2]) -> SIMD[DType.int8, 2 * n]:
    return x


def main():
    comptime count = mylen(Foo(3))
    print(count)
    print(lanes[2]())
    print(same[4]())
    print(doubled[1](SIMD[DType.int8, 2](1)))
