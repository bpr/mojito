# A trait requirement whose signature reads an element of the method's own
# pack (`Us[1]`, `TypeList[Us.values]()[1]`) matches a witness that reads
# the same element, whatever the witness calls its pack and after an
# earlier binder, and a call through the bound closes the element.
from std.builtin.variadics import TypeList


trait Getter:
    def get[*Us: AnyType](self, ref x: Us[1]) -> Int:
        ...

    def listed[*Us: AnyType](self, ref x: TypeList[Us.values]()[1]) -> Int:
        ...

    def after[T: AnyType, *Us: AnyType](self, ref x: Us[1]) -> Int:
        ...


struct S(Getter):
    def __init__(out self):
        pass

    def get[*Vs: AnyType](self, ref x: Vs[1]) -> Int:
        return 9

    def listed[*Us: AnyType](self, ref x: TypeList[Us.values]()[1]) -> Int:
        return 10

    def after[U: AnyType, *Vs: AnyType](self, ref x: Vs[1]) -> Int:
        return 11


def call[T: Getter](t: T) -> Int:
    var i = 3
    return t.get[Bool, Int](i)


def main():
    var s = S()
    var i = 3
    print(s.get[Bool, Int](i))
    print(s.listed[Bool, Int](i))
    print(s.after[Bool, Bool, Int](i))
    print(call(s))
