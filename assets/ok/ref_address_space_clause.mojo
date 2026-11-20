# A `ref[...]` specifier's operands are classified by type, as upstream's
# are: an `AddressSpace` operand is the reference's address space, in any
# position, and every other operand is an origin. A call deduces an
# `AddressSpace` binder from the place a `ref` argument lends, and a `ref`
# result carries its clause's space, as upstream's `Pointer.__getitem__`
# spells it (`-> ref[Self.origin, Self.address_space] Self.T`).
# Upstream prints `3`, `3`, `1`, `1`, `3`, `0`, `0`, `0`, `9`, and `9`.
from std.memory import AddressSpace


def g(ref[_, AddressSpace.GENERIC] x: Int) -> Int:
    return x


def h[o: Origin, a: AddressSpace](ref[o, a] x: Int) -> ref[o, a] Int:
    return x


def middle(ref a: Int, ref b: Int) -> ref[a, AddressSpace.GENERIC, b] Int:
    return a


def first(ref a: Int, ref b: Int) -> ref[AddressSpace.GENERIC, a] Int:
    return a


def alone(ref[AddressSpace.GENERIC] x: Int) -> Int:
    return x


def space[a: AddressSpace](ref[_, a] x: Int) -> Int:
    return Int(a)


@fieldwise_init
struct W[o: Origin, a: AddressSpace]:
    var p: Pointer[Int, Self.o, address_space = Self.a]

    def get(self) -> ref[Self.o, Self.a] Int:
        return self.p[]


def main():
    var y = 3
    var z = 1
    print(g(y))
    print(h(y))
    print(middle(z, y))
    print(first(z, y))
    print(alone(y))
    print(space(y))
    print(space(h(y)))
    ref r = y
    print(space(r))
    var w = W(Pointer(to=y))
    w.get() = 9
    print(w.get())
    print(y)
