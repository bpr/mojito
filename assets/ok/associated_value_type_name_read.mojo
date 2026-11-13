# A struct's compile-time value member read through the type name at run
# time: printed, passed to a constructor, in arithmetic, and as a run-time
# `TypeList` construction's bracket argument over a raw-list member.
from std.builtin.variadics import TypeList, _MLIR


struct E:
    comptime k = 3
    comptime types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, Int, Bool
    ].values


struct S[*Ts: AnyType]:
    comptime _types: _MLIR.KGENParamListType[AnyType] = Self.Ts.values


struct G:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def main():
    print(E.k)
    print(G(E.k).v)
    print(E.k + 1)
    var t = TypeList[Trait=AnyType, E.types]()
    var u = TypeList[Trait=AnyType, TypeList.of[Trait=AnyType, Int].values]()
    var v = TypeList[Trait=AnyType, S[Int, Bool]._types]()
    print(t.length, u.length, v.length)
