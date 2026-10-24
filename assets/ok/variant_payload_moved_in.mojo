# A `Variant`'s new payload is moved in, as upstream's `init_pointee_move`
# moves it: constructing one and `set` run no copy initializer, the value
# `set` replaces dies at the store, and the payload dies with the variant.
# A payload built from an explicit `copy()` is copied once.
from std.utils import Variant


struct D(Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 10
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)


struct CBox[n: Int](Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100


def main():
    var x = Variant[D, Int](D(1))
    print("built")
    x.set[D](D(2))
    print("set")
    _ = x^
    var b = CBox[1](1)
    var y = Variant[CBox[1], Int](b.copy())
    print(y[CBox[1]].v, b.v)
    print("end")
