# A field chain off a compile-time struct value read as a call argument:
# the chain folds through generic structs (`MO.g.s`) and only the leaf
# crosses to run time, an owned consumer (`var` parameter, owning
# collector) takes the folded value whole, and a borrower, a generic `T`,
# or a pack element reads it from a temporary.
@fieldwise_init
struct G[T: Copyable & Writable & Deinitable](Copyable, Movable):
    var v: Self.T
    var s: String


@fieldwise_init
struct Outer(Copyable, Movable):
    var g: G[Int]
    var name: String


@fieldwise_init
struct P(Copyable, Movable):
    var s: String
    var t: String


comptime MG = G[Int](11, "modg")
comptime MO = Outer(G[Int](12, "inner"), "outer")
comptime MP = P("x", "y")


def owns(var s: String):
    print(s)


def own[T: Writable & Copyable & Deinitable](var x: T):
    print(x)


def one[T: Writable](x: T):
    print(x)


def kw[T: Writable](*, x: T):
    print(x)


def many[*Ts: Writable](*xs: *Ts):
    comptime for i in range(xs.__len__()):
        print(xs[i])


def collect(var *xs: String):
    for x in xs:
        print(x)


def gen[n: Int]():
    comptime g = G[Int](n, "in gen")
    one(g.s)
    one(g.v)
    many(g.s, g.v, MO.g.s)
    comptime d = {"k": n}
    comptime for e in d.items():
        one(e.key)
        many(e.key, e.value)


def main():
    owns(MP.s)
    owns(MP.t)
    owns(MO.name)
    own(MP.s)
    own(MO.name)
    owns(MG.s)
    collect(MP.s, MO.name)
    comptime o = P("a", "b")
    owns(o.s)
    comptime for p in [P("c", "d")]:
        owns(p.s)
    one(MG.s)
    one(MG.v)
    one(MO.g.s)
    one(MO.g.v)
    many(MO.g.s)
    print(MO.g.s)
    print(MG.s, MO.g.v)
    many(MG.s, MO.g.v, MO.name)
    kw(x=MG.s)
    comptime q = Outer(G[Int](1, "z"), "w")
    print(q.g.s, q.name)
    comptime for r in [Outer(G[Int](2, "zz"), "ww")]:
        print(r.g.s)
    var a = MO.g.s
    print(a, MO.g.s.byte_length())
    gen[3]()
    comptime t = (1, "two")
    one(t[0])
    many(t[0], t[1])
    comptime for e in {"p": 1}.items():
        print(e.key, e.value)
        own(e.key)
    comptime l = [G[Int](1, "a"), G[Int](2, "b")]
    comptime for g in l.__iter__():
        one(g.s)
        print(g.v, g.s)
