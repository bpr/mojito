# Upstream's attribute fallback: a name no member of the receiver declares is
# the call `receiver.__getattr_param__["name"]()`, over a struct value, a
# struct type with a `@staticmethod` hook, a leading-dot member, a
# compile-time binding, a parameter argument and default, and a value of a
# type parameter whose bound requires the hook. A declared member
# (`Color.blue`, `AddressSpace.SHARED`) never reaches the hook.
# Upstream prints `1 7 9`, `42 44`, `41 42 43`, `Color(1) 1 0 Color(2)`, `13`,
# and `AddressSpace.SHARED`.
from std.memory import AddressSpace


struct S:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def __getattr_param__[name: StaticString](self) -> Int:
        comptime if name == "y":
            return self.x + 6
        else:
            return self.x + 8


struct T:
    @staticmethod
    def __getattr_param__[name: StaticString]() -> Int:
        return 40 + name.byte_length()


@fieldwise_init
struct Color(ImplicitlyCopyable, Writable):
    var v: Int

    comptime blue = Color(2)

    @staticmethod
    def __getattr_param__[name: StaticString]() -> Color:
        comptime if name == "red":
            return Color(1)
        else:
            return Color(0)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Color(", self.v, ")")


trait HasAttr:
    def __getattr_param__[name: StaticString](self) -> Int:
        ...


@fieldwise_init
struct A(HasAttr):
    var base: Int

    def __getattr_param__[name: StaticString](self) -> Int:
        return self.base + name.byte_length()


def f[n: Int]() -> Int:
    return n


def g[n: Int = T.abc]() -> Int:
    return n


def pick[c: Color]() -> Int:
    return c.v


def via[X: HasAttr](x: X) -> Int:
    return x.abc


def main():
    var s = S(1)
    print(s.x, s.y, s.z)
    print(T.ab, T.abcd)
    comptime c = T.q
    print(c, f[T.ab](), g())
    var col: Color = .red
    print(col, pick[.red](), pick[.green](), Color.blue)
    print(via(A(10)))
    print(AddressSpace.SHARED)
