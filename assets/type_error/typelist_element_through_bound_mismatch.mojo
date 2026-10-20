# expect: type mismatch for argument to 'first': expected Int, found Bool
# A `TypeList` element read through a bound closes to the conformer's
# element at the call, so a `Bool` does not pass where it is `Int`.
from std.builtin.variadics import TypeList, _MLIR


trait HasTypes:
    comptime _types: _MLIR.KGENParamListType[AnyType]


struct S[*Ts: AnyType](HasTypes):
    comptime _types: _MLIR.KGENParamListType[AnyType] = Self.Ts.values

    def __init__(out self):
        pass


def first[
    T: HasTypes
](ref x: TypeList[Trait=AnyType, T._types]()[0]) -> ref[x] TypeList[
    Trait=AnyType, T._types
]()[0]:
    return x


def main():
    var b = True
    print(first[S[Int, Bool]](b))
