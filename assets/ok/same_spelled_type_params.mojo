# Unrelated declarations that all spell their type parameter `T` are
# different parameters: a free generic bound to a struct's own, a struct
# holding another struct's instance, a generic method beside its struct's
# binder, swapped binder names across calls, and an associated type equal
# to the struct's binder against a def's own.
from std.hashlib import Hasher


trait Container:
    comptime Element: Copyable & Deinitable

    def first(self) -> Self.Element:
        ...


struct Box[T: Copyable & Deinitable](Copyable, Movable):
    var v: Self.T

    def __init__(out self, v: Self.T):
        self.v = v.copy()

    def get(self) -> Self.T:
        return self.v.copy()

    def rewrap[U: Copyable & Deinitable](self, u: U) -> Box[U]:
        return Box[U](u)


struct Outer[T: Copyable & Deinitable](Copyable, Movable):
    var label: Self.T
    var inner: Box[Int]

    def __init__(out self, label: Self.T, n: Int):
        self.label = label.copy()
        self.inner = Box[Int](n)

    def show(self) -> Int:
        return self.inner.get() + 1


struct Pair[T: Copyable & Deinitable, U: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T
    var b: Self.U

    def __init__(out self, a: Self.T, b: Self.U):
        self.a = a.copy()
        self.b = b.copy()


struct Cell[T: Copyable & Deinitable](Container, Copyable, Movable):
    comptime Element = Self.T
    var v: Self.T

    def __init__(out self, v: Self.T):
        self.v = v.copy()

    def first(self) -> Self.T:
        return self.v.copy()


def ident[T: Copyable & Deinitable](x: T) -> T:
    return x.copy()


def twice[T: Copyable & Deinitable](b: Box[T]) -> T:
    return ident(b.get())


def mk[T: Copyable & Deinitable, U: Copyable & Deinitable](a: T, b: U) -> Pair[T, U]:
    return Pair[T, U](a, b)


def flip[U: Copyable & Deinitable, T: Copyable & Deinitable](a: T, b: U) -> Pair[U, T]:
    return mk(b, a)


def take[T: Copyable & Deinitable, C: Container](c: C, fallback: T) -> C.Element:
    return c.first()


def main():
    print(twice(Box[Int](3)), twice(Box[String]("hi")))
    var o = Outer[String]("x", 41)
    print(o.show(), o.label)
    print(Box[Int](3).rewrap[String]("s").get(), Box[String]("t").rewrap(9).get())
    var p = flip(1, String("s"))
    print(p.a, p.b)
    print(take(Cell[String]("e"), 0), take(Cell[Int](5), String("f")))
