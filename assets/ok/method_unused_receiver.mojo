# A method whose body never uses its receiver leaves the generator a
# resolved method call names without it, as upstream's `RemoveUnusedParams`
# removes `self` like any other argument (`k3(q::Holder)_REMOVED_ARG()`):
# the call passes no receiver, and `Grid[n].k3` has one instance for every
# `n`, since nothing reads `n` once `self` is gone.


struct Holder(Copyable, Movable):
    var name: String

    def __init__(out self, name: String):
        self.name = name

    @no_inline
    def k3(self) -> Int:
        return 3

    @no_inline
    def inc(mut self, x: Int) -> Int:
        return x + 1

    @no_inline
    def named(self) -> String:
        return self.name


struct Grid[n: Int](Copyable, Movable):
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    @no_inline
    def k3(self) -> Int:
        return 3

    @no_inline
    def width(self) -> Int:
        return Self.n


def make(name: String) -> Holder:
    return Holder(name)


def main() raises:
    var h = Holder("h")
    print(h.k3(), h.inc(41), h.named())
    print(make("t").k3(), make("v").named())
    var g1 = Grid[1](7)
    var g2 = Grid[2](8)
    print(g1.k3(), g2.k3(), g1.width(), g2.width())
