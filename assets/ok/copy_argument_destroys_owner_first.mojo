# A copy initializer gives its copy its own storage, so an owner whose last
# use is the copy of its field into a `var` argument is destroyed once the
# copy is made, before the call, as at the pin: `own1(h.c)` prints `copy 2`,
# `del 2`, then `own1 2`. A copy keeps the loans its value carries:
# `var c = h.v` copies a view of `xs` that `c` still reads.
struct C(ImplicitlyCopyable):
    var a: Int

    def __init__(out self, a: Int):
        self.a = a

    def __init__(out self, *, copy: Self):
        print("copy", copy.a)
        self.a = copy.a

    def __deinit__(deinit self):
        print("del", self.a)


@fieldwise_init
struct H(ImplicitlyCopyable):
    var c: C


struct O:
    def __init__(out self):
        pass

    def own2(self, var x: C, var y: C):
        print("own2", x.a, y.a)


struct V[o: ImmOrigin](ImplicitlyCopyable):
    var s: Span[Int, Self.o]
    var tag: String

    def __init__(out self, s: Span[Int, Self.o], tag: String):
        self.s = s
        self.tag = tag

    def __init__(out self, *, copy: Self):
        print("copy", copy.tag)
        self.s = copy.s
        self.tag = copy.tag


@fieldwise_init
struct W[o: ImmOrigin](ImplicitlyCopyable):
    var v: V[Self.o]


def own1(var x: C):
    print("own1", x.a)


def main():
    var h = H(C(2))
    own1(h.c)
    var obj = O()
    var h1 = H(C(4))
    var h2 = H(C(5))
    obj.own2(y=h1.c, x=h2.c)
    var xs: List[Int] = [7, 8]
    var w = W(V(Span(xs), "t"))
    var c = w.v
    print("view", c.s[1], c.tag)
    print("end")
