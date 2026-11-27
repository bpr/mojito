# An origin binder a parameter names solves through the argument's
# implicit view conversion with its `mut=` binder: a `List` passed where a
# `Span[Byte, o]` is declared binds `o` to the list and `o`'s
# `Origin[mut=m]` binder to the list's mutability, as the converted
# value's type does — in a free call, a method call, an initializer's own
# parameters, and a struct's parameters a construction infers. An
# `ImmOrigin` binder bound so only reads its origin.
struct V[mut: Bool, //, T: Copyable, origin: Origin[mut=mut]]:
    var n: Int

    @implicit
    def __init__(out self, ref [Self.origin] xs: List[Self.T]):
        self.n = len(xs)


struct Holder[m: Bool, //, o: Origin[mut=m]]:
    var s: Span[Byte, Self.o]

    def __init__(out self, s: Span[Byte, Self.o]):
        self.s = s

    def is_mut(self) -> Bool:
        return Self.m


struct Tagged[T: Copyable & Deinitable, m: Bool, //, o: Origin[mut=m]]:
    var s: Span[Byte, Self.o]
    var t: Self.T

    def __init__(out self, s: Span[Byte, Self.o], t: Self.T):
        self.s = s
        self.t = t.copy()

    def __init__(out self, s: Span[Byte, Self.o], t: Self.T, extra: Int):
        self.s = s
        self.t = t.copy()

    def is_mut(self) -> Bool:
        return Self.m


struct Probe:
    var n: Int

    def __init__[m: Bool, //, o: Origin[mut=m]](out self, s: Span[Byte, o]):
        self.n = 1 if m else 0

    def reads[m: Bool, //, o: Origin[mut=m]](self, s: Span[Byte, o]) -> Bool:
        return m


def span_mut[m: Bool, //, o: Origin[mut=m]](s: Span[Byte, o]) -> Bool:
    return s.mut


def binder_mut[m: Bool, //, o: Origin[mut=m]](s: Span[Byte, o]) -> Bool:
    return m


def view_mut[m: Bool, //, o: Origin[mut=m]](v: V[Int, o]) -> Bool:
    return m


def both[o: ImmOrigin](a: Span[Byte, o], b: Span[Byte, o]) -> Int:
    return len(a) + len(b)


def from_read(xs: List[Byte]) -> Bool:
    return span_mut(xs)


def from_mut(mut xs: List[Byte]) -> Bool:
    return span_mut(xs)


def tagged_from_read(xs: List[Byte]) -> Bool:
    return Tagged(xs, 1).is_mut()


def main():
    var bs: List[Byte] = [1, 2, 3]
    var xs: List[Int] = [1, 2]
    print(span_mut(bs), binder_mut(bs), span_mut(Span(bs)))
    print(from_read(bs), from_mut(bs))
    print(view_mut(xs))
    print(Probe(bs).n, Probe(bs).reads(bs))
    print(Holder(bs).is_mut())
    print(Tagged(bs, 1).is_mut(), Tagged(bs, 1, 2).is_mut())
    print(tagged_from_read(bs))
    print(both(bs, bs))
