# A display, repr, hash, equality, `__int__`, writer `write_string`, or
# `__call__` member that reads no struct parameter is one instance for
# every instance of its struct, as on the pin: `WBox[1]` and `WBox[2]`
# share one `write_to`, `write_repr_to`, `__hash__`, `__eq__`, and
# `__int__`, `Adder[1]` and `Adder[2]` one `__call__`, and `MyW[1]` and
# `MyW[2]` one `write_string`. The backends find each through the struct
# declaration's member table, so printing, hashing, comparing, converting,
# writing, and calling a value still runs the member, not a reflective
# default.
from std.hash import Hasher


@fieldwise_init
struct WBox[n: Int](Writable, Hashable, Equatable, Intable, ImplicitlyCopyable):
    var v: Int

    @no_inline
    def write_to(self, mut writer: Some[Writer]):
        writer.write("W(", self.v, ")")

    @no_inline
    def write_repr_to(self, mut writer: Some[Writer]):
        writer.write("WBox(", self.v, ")")

    @no_inline
    def __hash__[H: Hasher](self, mut hasher: H):
        self.v.__hash__(hasher)

    @no_inline
    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v

    @no_inline
    def __int__(self) -> Int:
        return self.v


@fieldwise_init
struct Adder[n: Int](def(Int) -> Int, ImplicitlyCopyable):
    var v: Int

    @no_inline
    def __call__(self, x: Int) -> Int:
        return self.v + x


@fieldwise_init
struct MyW[n: Int](Writer):
    var count: Int

    @no_inline
    def write_string(mut self, string: StringSlice):
        self.count = self.count + string.byte_length() + 100


def apply[F: def(Int) -> Int](f: F, x: Int) -> Int:
    return f(x)


def main():
    var a = WBox[1](1)
    var b = WBox[2](2)
    print(a, b)
    print(repr(a), repr(b))
    print(hash(a) == hash(WBox[1](1)), hash(b) == hash(WBox[2](2)))
    print(a == WBox[1](1), b != WBox[2](2))
    print(Int(a), Int(b), Byte(a), UInt32(b))
    var f = Adder[1](10)
    var g = Adder[2](20)
    print(f(1), g(2), apply(f, 3), apply(g, 4))
    var w = MyW[1](0)
    w.write("ab", "c")
    var w2 = MyW[2](0)
    w2.write("b")
    print(w.count, w2.count)
