# `TypeList[...]` binds its bracket list as the bundled struct's, so it reads
# a raw list a struct's `comptime` member holds: concretely
# (`S[Int, Bool]._types`) and through a trait bound (`T._types`), with or
# without the keyword `Trait=` and the construction `()`.
from std.builtin.variadics import TypeList, _MLIR


trait HasTypes:
    comptime _types: _MLIR.KGENParamListType[AnyType]


struct S[*Ts: AnyType](HasTypes):
    comptime _types: _MLIR.KGENParamListType[AnyType] = Self.Ts.values

    def __init__(out self):
        pass


def count[T: HasTypes]() -> Int:
    return TypeList[Trait=AnyType, T._types]().length


def is_pair[T: HasTypes]() -> Bool:
    comptime if TypeList[T._types].length == 2:
        return True
    return False


def count_pack[*Ts: AnyType]() -> Int:
    return TypeList[Trait=AnyType, values=Ts.values]().length


def main():
    print(count[S[Int, Bool, NoneType]]())
    print(TypeList[Trait=AnyType, S[Int, Bool]._types]().length)
    print(TypeList[S[Int, Bool]._types].length)
    print(is_pair[S[Int, Bool]](), is_pair[S[Int]]())
    print(count_pack[Int, Bool, Int, Bool]())
