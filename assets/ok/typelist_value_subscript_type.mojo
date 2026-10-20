# A subscript of a `TypeList` construction is a type: it reads the
# construction's `__getitem_param__[i]`, in a parameter, a `ref` result, a
# local annotation over a struct member's raw list, a struct method over
# `Self.Ts.values`, a `rebind` target, and a `comptime for` body.
from std.builtin.variadics import TypeList, _MLIR


struct P[*Ts: AnyType]:
    comptime _types: _MLIR.KGENParamListType[AnyType] = Self.Ts.values


struct S[*Ts: AnyType]:
    var a: Int

    def __init__(out self):
        self.a = 41

    def get[
        id: Int
    ](
        ref self, ref x: TypeList[Trait=AnyType, Self.Ts.values]()[id]
    ) -> ref[x] TypeList[Trait=AnyType, Self.Ts.values]()[id]:
        return x


def second[
    *Ts: AnyType
](ref x: TypeList[Trait=AnyType, Ts.values]()[1]) -> ref[x] TypeList[
    Trait=AnyType, Ts.values
]()[1]:
    return x


def idx[
    i: Int, *Ts: AnyType
](ref x: TypeList[Ts.values]()[i]) -> ref[x] TypeList[Ts.values]()[i]:
    return x


def pick[
    i: Int, *Ts: ImplicitlyCopyable
](x: Int) -> TypeList[Trait=ImplicitlyCopyable, Ts.values]()[i]:
    return rebind[TypeList[Trait=ImplicitlyCopyable, Ts.values]()[i]](x)


def show[*Ts: ImplicitlyCopyable & Writable & Deinitable](*args: *Ts):
    comptime for i in range(TypeList[Ts.values].length):
        var e: TypeList[
            Trait=ImplicitlyCopyable & Writable & Deinitable, Ts.values
        ]()[i] = args[i]
        print(e)


def main():
    var n = 3
    var y: TypeList[Trait=AnyType, P[Bool, Int]._types]()[1] = 4
    var z: TypeList[P[Bool, Int]._types]()[0] = True
    print(second[Bool, Int](n), y, z)
    var b = True
    var m = 5
    print(idx[0, Bool, Int](b), idx[1, Bool, Int](m))
    var s = S[Int, Bool]()
    var t = False
    print(s.get[1](t))
    print(pick[1, Bool, Int](8))
    show(1, True)
