# A function's origin `mut=` binder is a value in its body, bound to the
# mutability of the origin each call infers: a pointer to a `var` binds
# `True`, one to a read-only argument `False`. A body reading it has one
# instance per mutability; a forwarding caller passes its own binder on, a
# method and a nested function read it alike, and a `where` clause over it
# is checked against the call's binding.
from std.os import abort


def g[mut: Bool, //, o: Origin[mut=mut]](x: Pointer[Int, o]) -> Bool:
    return mut


def forward[mut: Bool, //, o: Origin[mut=mut]](x: Pointer[Int, o]) -> Bool:
    return g(x)


def outer[mut: Bool, //, o: Origin[mut=mut]](x: Pointer[Int, o]) -> Bool:
    def inner() -> Bool:
        return mut

    return inner()


def only_mutable[
    mut: Bool, //, o: Origin[mut=mut]
](x: Pointer[Int, o]) -> Int where mut:
    return x[]


struct Probe:
    var v: Int

    def __init__(out self):
        self.v = 0

    def m[mut: Bool, //, o: Origin[mut=mut]](self, x: Pointer[Int, o]) -> Bool:
        return mut


def read_only(x: Int) -> Bool:
    return g(Pointer(to=x))


def read_only_forward(x: Int) -> Bool:
    return forward(Pointer(to=x))


def read_only_outer(x: Int) -> Bool:
    return outer(Pointer(to=x))


def read_only_method(x: Int) -> Bool:
    return Probe().m(Pointer(to=x))


def main():
    var a = 1
    if not g(Pointer(to=a)) or read_only(a):
        abort("function binder")
    if not forward(Pointer(to=a)) or read_only_forward(a):
        abort("forwarded binder")
    if not outer(Pointer(to=a)) or read_only_outer(a):
        abort("nested read")
    if not Probe().m(Pointer(to=a)) or read_only_method(a):
        abort("method binder")
    print(only_mutable(Pointer(to=a)))
