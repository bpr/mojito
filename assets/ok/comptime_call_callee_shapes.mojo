# A type or a parameter argument that applies a callable to a compile-time
# parameter, for every callee the parameter domain applies by name: a
# generic `def` given its own parameters, an overloaded one, one with a
# default, a keyword, or a `var` parameter, and a static method of a struct
# or of a generic struct's instance. `not` and a conditional expression
# compile to the parameter domain's own operators. Each is one value
# wherever it is spelled: a signature, a field type, a body, or a caller
# that binds the parameter.

def h(n: Int) -> Int:
    return n * 2

def twice[k: Int]() -> Int:
    return k * 2

def scale[k: Int](x: Int) -> Int:
    return k * x

def ov(n: Int) -> Int:
    return n * 2

def ov(b: Bool) -> Int:
    return 8

def arity(a: Int) -> Int:
    return a

def arity(a: Int, b: Int) -> Int:
    return a + b

comptime K = 3

def d(n: Int, m: Int = 3) -> Int:
    return n + m

def dk(n: Int, m: Int = K) -> Int:
    return n + m

def bump(var n: Int) -> Int:
    n += 1
    return n * 2

def flag[b: Bool]():
    print(b)

struct Flag[b: Bool]:
    var x: Int

    def __init__(out self):
        self.x = 0

    def show(self):
        flag[Self.b]()

struct S:
    @staticmethod
    def f(n: Int) -> Int:
        return n * 2

    @staticmethod
    def g(n: Int) -> Int:
        return n * 4

    @staticmethod
    def g(b: Bool) -> Int:
        return 2

    @staticmethod
    def t[k: Int]() -> Int:
        return k * 2

    @staticmethod
    def big(n: Int, limit: Int = 2) -> Bool:
        return n > limit

struct G[k: Int]:
    var v: SIMD[DType.int32, Self.w()]

    def __init__(out self):
        self.v = 5

    @staticmethod
    def w() -> Int:
        return Self.k * 2

    @staticmethod
    def times(n: Int) -> Int:
        return Self.k * n

    @staticmethod
    def big() -> Bool:
        return Self.k > 2

    def get(self) -> SIMD[DType.int32, Self.w()]:
        return self.v

    def same(self) -> SIMD[DType.int32, G[Self.k].w()]:
        return self.v

    def widen[m: Int](self) -> SIMD[DType.int32, Self.times(m)]:
        return SIMD[DType.int32, G[Self.k].times(m)](3)

    def flagged(self) -> Flag[not Self.big()]:
        return Flag[not Self.big()]()

def generic[n: Int]() -> SIMD[DType.int32, twice[n]()]:
    return SIMD[DType.int32, twice[n]()](7)

def keyword[n: Int]() -> SIMD[DType.int32, twice[k=n]()]:
    return SIMD[DType.int32, twice[n]()](7)

def nested[n: Int]() -> SIMD[DType.int32, twice[twice[n]()]()]:
    var v: SIMD[DType.int32, twice[twice[n]()]()] = 1
    return v

def mixed[n: Int]() -> SIMD[DType.int32, scale[2](n)]:
    return SIMD[DType.int32, scale[2](n)](6)

def overloaded[n: Int, b: Bool]() -> SIMD[DType.int32, ov(n) * ov(b)]:
    return SIMD[DType.int32, ov(n) * ov(b)](5)

def by_arity[n: Int]() -> SIMD[DType.int32, arity(n) * arity(n, n)]:
    return SIMD[DType.int32, arity(n) * arity(n, n)](4)

def defaulted[n: Int]() -> SIMD[DType.int32, d(n)]:
    return SIMD[DType.int32, d(n, 3)](7)

def keyed[n: Int]() -> SIMD[DType.int32, d(n, m=6)]:
    return SIMD[DType.int32, d(m=6, n=n)](7)

def constant_default[n: Int]() -> SIMD[DType.int32, dk(n)]:
    return SIMD[DType.int32, dk(n, 3)](2)

def owned[n: Int]() -> SIMD[DType.int32, bump(n)]:
    return SIMD[DType.int32, bump(n)](7)

def static[n: Int]() -> SIMD[DType.int32, S.f(n)]:
    return SIMD[DType.int32, S.f(n)](7)

def static_overloaded[n: Int, b: Bool]() -> SIMD[DType.int32, S.g(n) * S.g(b)]:
    return SIMD[DType.int32, S.g(n) * S.g(b)](5)

def static_generic[n: Int]() -> SIMD[DType.int32, S.t[n]()]:
    return SIMD[DType.int32, S.t[n]()](7)

def static_flag[n: Int]() -> Flag[S.big(n)]:
    return Flag[S.big(n, limit=2)]()

def on_instance[n: Int]() -> SIMD[DType.int32, G[n].w()]:
    return SIMD[DType.int32, G[n].w()](7)

def on_closed_instance[n: Int]() -> SIMD[DType.int32, G[2].times(n)]:
    return SIMD[DType.int32, G[2].times(n)](7)

def negated[n: Int]() -> Flag[not (n == 9)]:
    return Flag[not (n == 9)]()

# `not (a == b)` is `a != b`, and `not not b` is `b`.
def negated_spelling[n: Int]() -> Flag[not (h(n) == 8)]:
    return Flag[h(n) != 8]()

def twice_negated[b: Bool]() -> Flag[not not b]:
    return Flag[b]()

def chosen[n: Int]() -> SIMD[DType.int32, n if n > 2 else h(n)]:
    return SIMD[DType.int32, n if n > 2 else h(n)](7)

def chosen_literal[n: Int]() -> SIMD[DType.int32, 4 if n > 2 else 8]:
    return SIMD[DType.int32, 4 if n > 2 else 8](7)

def total[n: Int](v: SIMD[DType.int32, twice[n]()]) -> Int:
    return Int(v.reduce_add()) + n

def total_static[n: Int](v: SIMD[DType.int32, S.f(n)]) -> Int:
    return n

def total_instance[n: Int](v: SIMD[DType.int32, G[n].w()]) -> Int:
    return n

def total_default[n: Int](v: SIMD[DType.int32, d(n)]) -> Int:
    return n

def total_chosen[n: Int](v: SIMD[DType.int32, n if n > 2 else h(n)]) -> Int:
    return n

struct W[n: Int]:
    var v: SIMD[DType.int32, twice[Self.n]()]
    var s: SIMD[DType.int32, S.f(Self.n)]
    var f: Flag[not S.big(Self.n)]

    def __init__(out self):
        self.v = 3
        self.s = 4
        self.f = Flag[not S.big(Self.n)]()

    def get(self) -> SIMD[DType.int32, S.f(Self.n) if S.big(Self.n) else 2]:
        return SIMD[DType.int32, S.f(Self.n) if S.big(Self.n) else 2](9)

def local[n: Int]() -> Int:
    comptime w = twice[n]()
    comptime b = not (n == 2)
    comptime c = w if b else n
    var v: SIMD[DType.int32, w] = 1
    var u: SIMD[DType.int32, c] = 2
    flag[b]()
    print(v, u)
    return w

def main():
    var v = generic[2]()
    print(v)
    var e: SIMD[DType.int32, twice[2]()] = generic[2]()
    print(e)
    print(total[2](v))
    print(total(v))
    print(keyword[2]())
    print(nested[1]())
    print(mixed[2]())
    var m: SIMD[DType.int32, scale[2](2)] = mixed[2]()
    print(m)
    print(overloaded[1, True]())
    var o: SIMD[DType.int32, ov(1) * ov(True)] = overloaded[1, True]()
    print(o)
    print(by_arity[2]())
    var x = defaulted[1]()
    print(x)
    print(total_default(x))
    var x1: SIMD[DType.int32, d(1)] = defaulted[1]()
    var x2: SIMD[DType.int32, d(1, 3)] = defaulted[1]()
    print(x1, x2)
    print(keyed[2]())
    var x3: SIMD[DType.int32, d(2, 6)] = keyed[2]()
    print(x3)
    print(constant_default[1]())
    print(owned[1]())
    var s = static[2]()
    print(s)
    print(total_static(s))
    var s1: SIMD[DType.int32, S.f(2)] = static[2]()
    print(s1)
    print(static_overloaded[1, True]())
    print(static_generic[2]())
    static_flag[1]().show()
    static_flag[3]().show()
    var i = on_instance[2]()
    print(i)
    print(total_instance[2](i))
    print(total_instance(i))
    print(on_closed_instance[2]())
    negated[1]().show()
    negated[9]().show()
    var n1: Flag[not (1 == 9)] = negated[1]()
    var n2: Flag[True] = negated[1]()
    n1.show()
    n2.show()
    negated_spelling[1]().show()
    twice_negated[True]().show()
    print(chosen[1]())
    print(chosen[4]())
    var c1: SIMD[DType.int32, 4] = chosen[4]()
    var c2: SIMD[DType.int32, h(1)] = chosen[1]()
    print(c1, c2)
    print(total_chosen[4](chosen[4]()))
    var c3: SIMD[DType.int32, 8] = chosen_literal[1]()
    print(c3)
    var g = G[2]()
    print(g.v)
    print(g.get())
    print(g.same())
    var g1: SIMD[DType.int32, G[2].w()] = g.get()
    print(g1)
    print(g.widen[2]())
    g.flagged().show()
    G[4]().flagged().show()
    var w = W[1]()
    print(w.v, w.s)
    w.f.show()
    print(w.get())
    print(W[4]().get())
    print(local[2]())
    print(local[4]())
