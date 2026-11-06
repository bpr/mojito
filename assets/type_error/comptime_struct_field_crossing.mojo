# expect: cannot materialize comptime value of type 'G
# A field folded off a compile-time struct value crosses to run time only
# when its type is implicitly copyable: `MO.g.s` crosses, but `MO.g`, a
# `G[Int]`, would materialize whole, which the pin rejects.
@fieldwise_init
struct G[T: Copyable & Writable & Deinitable](Copyable, Movable):
    var v: Self.T
    var s: String


@fieldwise_init
struct Outer(Copyable, Movable):
    var g: G[Int]
    var name: String


comptime MO = Outer(G[Int](12, "inner"), "outer")


def r(x: G[Int]):
    print(x.s)


def main():
    print(MO.g.s)
    r(MO.g)
