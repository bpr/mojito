# A call of an `@always_inline("builtin")` method with `self` in a parameter
# expression is its body's expression, `self` and a `Self`-typed argument
# bound to the receiver and the operand: `L(n).doubled()` over `return
# self.v + self.v` is `2 * n`, `l.doubled()` over a binder reads `l.v`, a
# struct operator applies its builtin dunder (`(L(n) + L(1)).get()` is
# `n + 1`, `1 + L(n)` the reflected one), a generic struct's method reads
# the instance's parameters, a call through a struct parameter
# (`Self.l.doubled()`) folds too, and a comparison dunder decides a `where`
# clause and a `comptime if`, as the pinned Mojo folds them.
struct L(TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        self.v = v

    @always_inline("builtin")
    @staticmethod
    def mk(v: Int) -> Self:
        return Self(v + 1)

    @always_inline("builtin")
    def doubled(self) -> Int:
        return self.v + self.v

    @always_inline("builtin")
    def get(imm self) -> Int:
        return self.v

    @always_inline("builtin")
    def plus2(self) -> Int:
        return self.doubled() + 2

    @always_inline("builtin")
    def scaled[k: Int](self) -> Int:
        return self.v * k

    @always_inline("builtin")
    def __add__(self, rhs: Self) -> Self:
        return Self(self.v + rhs.v)

    @always_inline("builtin")
    def __radd__(self, lhs: Int) -> Self:
        return Self(lhs + self.v)

    @always_inline("builtin")
    def __neg__(self) -> Self:
        return Self(-self.v)

    @always_inline("builtin")
    def __lt__(self, rhs: Self) -> Bool:
        return self.v < rhs.v


struct G[k: Int](TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        self.v = v

    @always_inline("builtin")
    def __add__(self, rhs: Self) -> Self:
        return Self(self.v + rhs.v + Self.k)

    @always_inline("builtin")
    def get(self) -> Int:
        return self.v


struct Tagged[l: L](ImplicitlyCopyable):
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.l.v

    def widen(self, x: SIMD[DType.int8, Self.l.doubled()]) -> SIMD[DType.int8, 2 * Self.l.v]:
        return x


def doubled[n: Int](x: SIMD[DType.int8, L(n).doubled()]) -> SIMD[DType.int8, 2 * n]:
    return x


def bound[l: L](x: SIMD[DType.int8, l.doubled()]) -> SIMD[DType.int8, 2 * l.v]:
    return x


def summed[n: Int](x: SIMD[DType.int8, (L(n) + L(1)).get()]) -> SIMD[DType.int8, n + 1]:
    return x


def reflected[n: Int](x: SIMD[DType.int8, (1 + L(n)).get()]) -> SIMD[DType.int8, n + 1]:
    return x


def negated[n: Int](x: Tagged[-L(n)]) -> Tagged[L(-n)]:
    return x


def chained[n: Int](x: SIMD[DType.int8, L(n).plus2()]) -> SIMD[DType.int8, 2 * n + 2]:
    return x


def scaled[n: Int](x: SIMD[DType.int8, L(n).scaled[2]()]) -> SIMD[DType.int8, 2 * n]:
    return x


def made[n: Int](x: SIMD[DType.int8, L.mk(n).v]) -> SIMD[DType.int8, n + 1]:
    return x


def generic[n: Int](x: SIMD[DType.int8, (G[1](n) + G[1](1)).get()]) -> SIMD[DType.int8, n + 2]:
    return x


def ordered[n: Int](x: SIMD[DType.int8, n]) -> SIMD[DType.int8, n] where L(n) < L(3):
    return x


def tagged[n: Int](x: Tagged[L(n) + L(1)]) -> Tagged[L(n + 1)]:
    return x


def branch[n: Int]() -> Int:
    comptime if L(n) < L(3):
        return (L(n) + L(1)).get()
    else:
        return -1


def main():
    print(doubled[1](SIMD[DType.int8, 2](1)))
    print(bound[L(1)](SIMD[DType.int8, 2](2)))
    print(summed[1](SIMD[DType.int8, 2](3)))
    print(reflected[1](SIMD[DType.int8, 2](4)))
    print(negated[2](Tagged[L(-2)]()).get())
    print(chained[1](SIMD[DType.int8, 4](6)))
    print(scaled[1](SIMD[DType.int8, 2](7)))
    print(made[1](SIMD[DType.int8, 2](8)))
    print(generic[2](SIMD[DType.int8, 4](9)))
    print(ordered[2](SIMD[DType.int8, 2](10)))
    print(Tagged[L(1)]().widen(SIMD[DType.int8, 2](11)))
    print(tagged[1](Tagged[L(2)]()).get(), branch[1](), branch[4]())
    var l = L(3)
    print(l.doubled(), (l + L(4)).get(), l.plus2(), l.scaled[5](), l < L(4))
    comptime c = L(2) + L(5)
    var held = c
    print(held.v, Tagged[L(1) + L(1)]().get(), G[3](1).__add__(G[3](2)).get())
