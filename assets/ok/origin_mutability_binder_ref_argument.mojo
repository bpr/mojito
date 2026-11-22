# A `ref[o]` argument binds its origin binder to the place it lends and the
# binder's `mut=` binder to that place's mutability, as a pointer argument
# does: a `var` binds `True`, a read-only argument `False`. The body reads
# the binder by name, through `o.mut`, and through a pointer to the
# parameter; a forwarding caller passes its own binder on, and a method, a
# static method, an initializer, an operator, and a parametric struct's
# witness of a trait requirement bind it alike.
from std.memory import Pointer


def g[m: Bool, //, o: Origin[mut=m]](ref[o] x: Int) -> Bool:
    return m


def through_origin[m: Bool, //, o: Origin[mut=m]](ref[o] x: Int) -> Bool:
    return o.mut


def through_pointer[m: Bool, //, o: Origin[mut=m]](ref[o] x: Int) -> Bool:
    return Pointer(to=x).mut


def forward[m: Bool, //, o: Origin[mut=m]](ref[o] x: Int) -> Bool:
    return g(x)


trait Peek:
    def peek[m: Bool, //, o: Origin[mut=m]](self, ref[o] x: Int) -> Bool:
        ...


struct Probe[T: Copyable](Peek):
    var b: Bool

    def __init__(out self):
        self.b = False

    def __init__[m: Bool, //, o: Origin[mut=m]](out self, ref[o] x: Int):
        self.b = m

    def peek[m: Bool, //, o: Origin[mut=m]](self, ref[o] x: Int) -> Bool:
        return m

    @staticmethod
    def st[m: Bool, //, o: Origin[mut=m]](ref[o] x: Int) -> Bool:
        return m

    def __add__[m: Bool, //, o: Origin[mut=m]](self, ref[o] rhs: Int) -> Bool:
        return m


def report(x: Int):
    print(
        g(x),
        through_origin(x),
        through_pointer(x),
        forward(x),
        Probe[Int]().peek(x),
        Probe[Int].st(x),
        Probe[Int](x).b,
        Probe[Int]() + x,
    )


def main():
    var a = 1
    print(
        g(a),
        through_origin(a),
        through_pointer(a),
        forward(a),
        Probe[Int]().peek(a),
        Probe[Int].st(a),
        Probe[Int](a).b,
        Probe[Int]() + a,
    )
    report(a)
    var p = Pointer(to=a)
    print(g(p[]))
