# A generic body that only reads a `T` or a pack element `Ts[i]` copies
# nothing, as upstream passes a memory-only value by address: `print`, the
# `Bool`/`String`/`repr`/`Int` conversions, a bound comparison, a condition,
# and an ordinary `read` parameter each read a pack element where it lies;
# hashing a `List` or `Array`, `in` over a `Tuple`, and `write_repr_to` on an
# element reached through a bound read each element in place, and so does a
# `Byte` built from an `Intable` parameter.
from std.hash import Hasher


struct Tally(Copyable, Writable, Defaultable, Equatable, Boolable, Hashable, Intable):
    var v: Int

    def __init__(out self):
        self.v = 1

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        print("  copy", copy.v)
        self.v = copy.v

    def __deinit__(deinit self):
        print("  del", self.v)

    def write_to(self, mut w: Some[Writer]):
        w.write("T", self.v)

    def write_repr_to(self, mut w: Some[Writer]):
        w.write("Tally(", self.v, ")")

    def __int__(self) -> Int:
        return self.v

    def __bool__(self) -> Bool:
        return self.v != 0

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v

    def __hash__[H: Hasher](self, mut hasher: H):
        self.v.__hash__(hasher)


def show[T: Writable](x: T):
    print("  show", x)


def pack_reads[
    *Ts: Writable & Defaultable & Equatable & Boolable & Intable & Deinitable
]():
    comptime for i in range(len(Ts)):
        comptime T = Ts[i]
        var value: T = T()
        var other: T = T()
        print(value)
        if value:
            print("  truthy")
        print(Bool(value), String(value), repr(value), Int(value))
        print(value == other)
        show(value)


def repr_first[T: Writable & Copyable](items: List[T]) -> String:
    var text = String()
    items[0].write_repr_to(text)
    return text


def to_byte[T: Intable](x: T) -> Byte:
    return Byte(x)


def main():
    print("pack")
    pack_reads[Tally, Int]()
    var items = List[Tally]()
    items.append(Tally(2))
    items.append(Tally(3))
    print("hash list")
    _ = hash(items)
    var pair: Array[Tally, 2] = [Tally(4), Tally(5)]
    print("hash array")
    _ = hash(pair)
    var t = (Tally(6), 7)
    var probe = Tally(6)
    print("contains")
    print(probe in t)
    print("repr", repr_first(items))
    print("byte", to_byte(probe))
    print("end")
