# A per-instantiation method clone inherits its checked template's facts when
# the body moves a whole value of a parameter type (`docs/notes/
# instantiation-from-template.md`, class MethodBody, feature `opaque_moves`):
# a `var` parameter, a `^` transfer, a store to a field of the value's own
# type, a local, a result, and an `__init__`. The value is never an operand, a receiver, or
# an argument, so nothing dispatches on its type; an instance owes `Movable`
# at each transfer, the copy at each copied place, and its own judgment of
# whether a local can be destroyed.
@fieldwise_init
struct Token(Movable):
    var id: Int


@fieldwise_init
struct Slot[T: Movable & Deinitable](Movable):
    var item: Self.T
    var uses: Int

    def replace(mut self, var item: Self.T):
        self.item = item^
        self.uses += 1

    def cycle(mut self, var item: Self.T) -> Int:
        var held = item^
        self.item = held^
        return self.uses

    def take(deinit self) -> Self.T:
        return self.item^


# An `__init__` is an owned receiver like any other: which fields it
# initializes is its syntax. Both overloads derive.
struct Cell[T: Movable & Deinitable](Movable):
    var item: Self.T
    var uses: Int

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.uses = 0

    def __init__(out self, var item: Self.T, uses: Int):
        self.item = item^
        if uses < 0:
            self.uses = 0
        else:
            self.uses = uses

    def count(self) -> Int:
        return self.uses


# `T` carries no `Deinitable` bound, so `held` is a linear binding in the
# template; each instance judges it again at its own type.
@fieldwise_init
struct Keep[T: Movable](Movable):
    var uses: Int

    def pass_through(mut self, var item: Self.T) -> Self.T:
        var held = item^
        self.uses += 1
        return held^


@fieldwise_init
struct Pair[T: ImplicitlyCopyable & Deinitable](Copyable):
    var first: Self.T
    var second: Self.T

    def left(self) -> Self.T:
        return self.first

    def pick(self, right: Bool) -> Self.T:
        if right:
            return self.second
        var chosen = self.first
        return chosen^

    def flip(mut self):
        var old = self.first
        self.first = self.second
        self.second = old^


def main():
    var a = Slot(1, 0)
    a.replace(2)
    print(a.cycle(3), a^.take())
    var b = Slot(String("x"), 0)
    b.replace(String("y"))
    print(b.cycle(String("z")), b^.take())
    var c = Slot(Token(7), 0)
    c.replace(Token(8))
    print(c.cycle(Token(9)), c^.take().id)
    var d = Cell(1)
    var e = Cell(String("x"), 4)
    var f = Cell(2, -3)
    print(d.count(), e.count(), f.count(), d.item, e.item)
    var k = Keep[Int](0)
    var s = Keep[String](0)
    print(k.pass_through(4), s.pass_through(String("q")), k.uses + s.uses)
    var p = Pair(1, 2)
    p.flip()
    print(p.left(), p.pick(True), p.pick(False))
    var q = Pair(String("l"), String("r"))
    q.flip()
    print(q.left(), q.pick(True), q.pick(False))
