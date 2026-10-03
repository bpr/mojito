# An overloaded method with a binder of its own is declared under the symbol
# its calls name: a callable-bounded overload beside a plain one, a
# `Some[Writer]` `write_to` beside a generic rival reached from another
# `write_to`, and a `__hash__[H: Hasher]` beside a generic rival reached
# through a generic struct's `Hashable` bound. A `write_to` beside a rival
# taking a `String` displays through a `Writable` bound by the `Writer` one.
from std.hashlib import Hasher


def inc(x: Int) -> Int:
    return x + 1


@fieldwise_init
struct Box(Copyable, Movable):
    var v: Int

    def apply[F: def(x: Int) -> Int](self, f: F) -> Int:
        return f(self.v)

    def apply(self, x: Int) -> Int:
        return x + self.v


@fieldwise_init
struct Twin(Copyable, Deinitable, Hashable, Movable, Writable):
    var x: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("T", self.x)

    def write_to[U: Copyable](self, mut writer: List[U]):
        pass

    def __hash__[H: Hasher](self, mut hasher: H):
        self.x.__hash__(hasher)

    def __hash__[U: Copyable](self, mut hasher: List[U]):
        pass


@fieldwise_init
struct Wrap(Copyable, Movable, Writable):
    var item: Twin

    def write_to(self, mut writer: Some[Writer]):
        self.item.write_to(writer)


struct Holder[T: Copyable & Deinitable & Hashable & Writable](
    Hashable, Movable, Writable
):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def write_to(self, mut writer: Some[Writer]):
        self.item.write_to(writer)

    def __hash__[H: Hasher](self, mut hasher: H):
        self.item.__hash__(hasher)


@fieldwise_init
struct Rival(Copyable, Movable, Writable):
    var x: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("R", self.x)

    def write_to(self, mut writer: String):
        writer += "S"


def show[T: Writable](value: T):
    print(value)


def main():
    var b = Box(3)
    print(b.apply(inc), b.apply(10))
    print(Wrap(Twin(4)))
    print(Holder[Twin](Twin(5)))
    print(hash(Holder[Twin](Twin(6))) == hash(Twin(6)))
    show(Rival(7))
