# `TypeList.of` is the bundled struct's upstream alias, `comptime
# of[Trait: type_of(AnyType), //, *Ts: Trait] = TypeList[Trait=Trait,
# Ts.values]`, read through the unbound `TypeList`: its `.values` is a raw
# list (a struct member's, a construction's bracket argument), a construction
# through it is a `TypeList` value or, subscripted, a type, and a template
# applies it to its own parameters, in a function body or at module scope.
from std.builtin.variadics import TypeList, _MLIR

comptime Three = TypeList.of[Trait=AnyType, NoneType, Int, Bool]
comptime pair_list = TypeList.of[Trait=AnyType, NoneType, Int]()


struct E:
    comptime types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Int
    ].values


def count[T: AnyType, U: AnyType]() -> Int:
    return TypeList.of[Trait=AnyType, T, U].length


def pair[T: AnyType, U: AnyType]() -> Bool:
    comptime if TypeList.of[Trait=AnyType, T, U]().length == 2:
        return True
    return False


def main():
    print(TypeList[Trait=AnyType, E.types]().length)
    var y: TypeList[
        Trait=AnyType, TypeList.of[Trait=AnyType, Bool, Int].values
    ]()[1] = 4
    var z: TypeList.of[Trait=AnyType, Bool, Int]()[1] = 5
    print(y, z)
    comptime L = TypeList.of[Trait=AnyType, NoneType, Int, Bool]
    print(L.length)
    comptime l = TypeList.of[Trait=AnyType, NoneType, Int]()
    print(l.length, len(l))
    print(TypeList.of[Trait=AnyType, NoneType, Int, Bool, Int]().length)
    print(count[Int, Bool](), pair[Int, Bool]())
    print(Three.length, pair_list.length)
