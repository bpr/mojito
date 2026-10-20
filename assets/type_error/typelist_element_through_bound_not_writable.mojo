# expect: does not conform to trait 'Writable'
# A `TypeList` element read through a bound's `AnyType` raw-list member has
# only the list's trait in the body, so it does not print.
from std.builtin.variadics import TypeList, _MLIR


trait HasTypes:
    comptime _types: _MLIR.KGENParamListType[AnyType]


struct S[*Ts: AnyType](HasTypes):
    comptime _types: _MLIR.KGENParamListType[AnyType] = Self.Ts.values

    def __init__(out self):
        pass


def show[T: HasTypes](x: TypeList[Trait=AnyType, T._types]()[0]):
    print(x)


def main():
    show[S[Int, Bool]](3)
