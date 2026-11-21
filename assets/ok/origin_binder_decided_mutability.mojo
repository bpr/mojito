# A `mut=` value an origin binder decides is the binder's read in a checked
# instance, as upstream's instance carries it: `Span[Int, o]` in a signature
# generic over `m: Bool, //, o: Origin[mut=m]` is `Span[m, Int, o]`, and a
# call binds `m` (a free function's, a method's, a static method's, and a
# constructor's), so the instance a body reflects is `Span[True, …]` or
# `Span[False, …]`, never the binder's name. A struct's `where Self.mut`
# reads the receiver's `mut=` argument.


def f[m: Bool, //, o: Origin[mut=m]](s: Span[Int, o]):
    print(reflect[type_of(s)].name())


def imm[o: ImmOrigin](s: Span[Int, o]):
    f(s)


def g[m: Bool, //, o: Origin[mut=m]](s: Span[Int, o]) -> Span[Int, o]:
    return s


def h[m: Bool, //, o: Origin[mut=m]](s: Span[Int, o]) -> Int:
    var t = g(s)
    return len(t)


struct W:
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    def m[mm: Bool, //, o: Origin[mut=mm]](self, s: Span[Int, o]) -> Span[Int, o]:
        return s

    @staticmethod
    def st[mm: Bool, //, o: Origin[mut=mm]](s: Span[Int, o]) -> Span[Int, o]:
        return s


struct Holder[mut: Bool, //, o: Origin[mut=mut]]:
    var s: Span[Int, Self.o]

    def __init__[mm: Bool, //, p: Origin[mut=mm]](out self: Holder[p], s: Span[Int, p]):
        self.s = s


def through_methods[m: Bool, //, o: Origin[mut=m]](s: Span[Int, o]) -> Int:
    var w = W(1)
    return len(w.m(s)) + len(W.st(s))


struct Q[mut: Bool, //, o: Origin[mut=mut]]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def get(self) -> Int where Self.mut:
        return self.n


def through_mut_origin[o: MutOrigin](q: Q[o]) -> Int:
    return q.get()


def main():
    var xs: List[Int] = [1, 2]
    f(Span(xs))  # std.collections.span.Span[True, {}, SIMD[DType.int, 1], {}, 0]
    imm(Span(xs))  # std.collections.span.Span[False, {}, SIMD[DType.int, 1], {}, 0]
    var t = g(Span(xs))
    print(reflect[type_of(t)].name())  # ...Span[True, ...]
    print(h(Span(xs)))  # 2
    t[0] = 5
    print(xs[0])  # 5
    var w = W(1)
    var u = w.m(Span(xs))
    print(reflect[type_of(u)].name())  # ...Span[True, ...]
    u[1] = 7
    print(xs[1])  # 7
    print(len(W.st(Span(xs))))  # 2
    print(through_methods(Span(xs)))  # 4
    var held = Holder(Span(xs))
    print(len(held.s))  # 2
    var y = 1
    print(Q[MutUntrackedOrigin](4).get())  # 4
    print(Q[origin_of(y)](3).get())  # 3
    print(through_mut_origin(Q[origin_of(y)](6)))  # 6
