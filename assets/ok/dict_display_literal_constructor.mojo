# A non-empty dict display is `Dict`'s `__dict_literal__` constructor over
# a key list and a value list, as the pinned Mojo lowers it: each element
# moves into its list, and the constructor copies every entry into a pair
# and the pair's halves into the table, then destroys the pair and, last,
# the lists. The display and the spelled constructor call trace alike.
from std.hash import Hasher


@fieldwise_init
struct C(Equatable, Hashable, ImplicitlyCopyable, Movable):
    var v: Int

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 10
        print("copy", copy.v)

    def __deinit__(deinit self):
        print("del", self.v)

    def __eq__(self, o: Self) -> Bool:
        return self.v == o.v

    def __hash__[H: Hasher](self, mut h: H):
        self.v.__hash__(h)


def mk(v: Int) -> C:
    print("mk", v)
    return C(v)


def main():
    print("display")
    var e = {1: mk(4), 2: mk(7)}
    print(len(e))
    print("spelled")
    var h = Dict[Int, C](
        keys=[1, 2], values=[mk(4), mk(7)], __dict_literal__=None
    )
    print(len(h))
    print("traced key")
    var k = {mk(5): mk(6)}
    print(len(k))
    print("done")
