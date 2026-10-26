# Compile-time struct values: one built by a non-fieldwise initializer,
# and a generic struct's fieldwise construction.
struct R(Copyable, Movable):
    var a: Int
    var s: String

    def __init__(out self, a: Int):
        self.a = a
        self.s = String(a)


@fieldwise_init
struct G[T: Copyable & Writable & Deinitable](Copyable, Movable):
    var v: Self.T
    var s: String


def main():
    comptime r = R(3)
    print(r.a, r.s)
    comptime g = G[Int](5, "gen")
    print(g.v, g.s)
