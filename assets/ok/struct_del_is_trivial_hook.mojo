# A struct decides its own destructor's triviality through a declared
# `comptime __del__is_trivial` member, as upstream's `VariadicList` does over
# its `is_owned` parameter: `IsTriviallyDeinitable[Holder[False]]` holds
# though `Holder` declares a `__deinit__`, and `Holder[True]`'s does not.
from std.traits import IsTriviallyDeinitable


struct Holder[flag: Bool](Movable):
    var x: Int
    comptime __del__is_trivial: Bool = not Self.flag

    def __init__(out self, x: Int):
        self.x = x

    def __deinit__(deinit self):
        comptime if Self.flag:
            print("deinit", self.x)


def show[T: AnyType]():
    comptime if IsTriviallyDeinitable[T]:
        print("trivial")
    else:
        print("not trivial")


def main():
    var owned = Holder[True](1)
    var plain = Holder[False](2)
    print(owned.x + plain.x)
    show[Holder[True]]()
    show[Holder[False]]()
    comptime if Holder[False].__del__is_trivial:
        print("hook true")
