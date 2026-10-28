# A call of an `@always_inline("builtin")` `__init__` in a parameter
# expression is the struct value its body stores, field by field, so a field
# read through it folds (`L(n).v * 2` is `2 * n`), two constructions are one
# value when their fields are (`L(n, 0)` is `L(n)`), and a generic struct's
# body reads the instance's parameters (`G[1](n).v` is `n + 1`).
struct L(TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        self.v = v

    @always_inline("builtin")
    def __init__(out self, v: Int, w: Int):
        self.v = v + w


struct L2(TrivialRegisterPassable):
    var a: Int
    var b: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        comptime assert True
        var w = v * 2
        self.a = v
        self.b = self.a + 1
        self.b = w


struct G[k: Int](TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        self.v = v + Self.k


struct Tagged[l: L](ImplicitlyCopyable):
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.l.v


def doubled[n: Int](x: SIMD[DType.int8, L(n).v * 2]) -> SIMD[DType.int8, 2 * n]:
    return x


def stored[n: Int](x: SIMD[DType.int8, L2(n).b]) -> SIMD[DType.int8, 2 * n]:
    return x


def offset[n: Int](x: SIMD[DType.int8, G[1](n).v]) -> SIMD[DType.int8, n + 1]:
    return x


def same[n: Int](x: Tagged[L(n, 0)]) -> Tagged[L(n)]:
    return x


def closed(x: Tagged[L(1, 1)]) -> Tagged[L(2)]:
    return x


def inferred[n: Int](x: Tagged[L(n)]) -> Int:
    return n


def read[l: L](x: SIMD[DType.int8, l.v]) -> SIMD[DType.int8, l.v]:
    return x


def local[n: Int](x: SIMD[DType.int8, n]) -> SIMD[DType.int8, n]:
    comptime c = L(n)
    var y: SIMD[DType.int8, c.v] = x
    var held = c
    print(held.v, Tagged[L(n)]().get())
    return read[L(n)](y)


def main():
    print(doubled[1](SIMD[DType.int8, 2](1)))
    print(stored[1](SIMD[DType.int8, 2](2)))
    print(offset[1](SIMD[DType.int8, 2](3)))
    print(same[3](Tagged[L(3)]()).get())
    print(closed(Tagged[L(2)]()).get())
    print(inferred(Tagged[L(4)]()))
    print(local[2](SIMD[DType.int8, 2](5)))
    comptime d = L(6)
    print(d.v, Tagged[d]().get(), G[2](3).v)
