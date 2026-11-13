# A type annotation reads a member through a local compile-time value, as
# the pin types the base as an expression and reads the member on its type:
# a struct's type member, and `TypeList`'s parameterized
# `__getitem_param__`, whose `SIMDLength` index binds at its declared type.
from std.builtin.variadics import TypeList


struct W[n: Int]:
    comptime E = Bool

    def __init__(out self):
        pass


def main():
    comptime w = W[3]()
    var a: w.E = True
    print(a)
    comptime L = TypeList.of[Trait=AnyType, Int, Bool]()
    var m: L.__getitem_param__[SIMDLength(0)] = 3
    print(m)
    var b: L.__getitem_param__[1] = False
    print(b)
    var c: TypeList.of[Trait=AnyType, Int, Bool]().__getitem_param__[
        SIMDLength(1)
    ] = True
    print(c)
