# A type pack read as a value is the zero-sized `TypeList[Ts.values]()` the
# pin types it as, `Self.Ts` included: it binds as one value, and `len` of it
# is the library `len` calling `TypeList.__len__`. A `comptime` binding of a
# `TypeList` value is one too, read at run time as the list it binds.
from std.builtin.variadics import TypeList


def show[T: Sized](x: T) -> Int:
    return len(x)


def go[*Ts: Movable]():
    var x = Ts
    print(len(x), x.length)
    comptime tl: TypeList[Trait=Movable, Ts.values] = Ts
    print(tl.length, len(tl))
    print(len(Ts), show(Ts))
    comptime again = Ts
    if len(x) > 0:
        var y = again
        print(len(y))


struct Many[*Ts: Movable]:
    def __init__(out self):
        pass

    def count(self) -> Int:
        return len(Self.Ts)

    @staticmethod
    def length() -> Int:
        var l = Self.Ts
        return len(l)


def main():
    go[Int, Bool, String]()
    print(Many[Int, Bool]().count(), Many[Int].length())
    comptime types = TypeList.of[Trait=AnyType, Int, Bool]()
    var t = types
    print(len(t), len(types), show(types))
