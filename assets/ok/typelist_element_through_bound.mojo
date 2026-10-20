# A `TypeList` element read through a bound's raw-list member keeps its
# dependent type in the body, with the trait the member's list declares as
# its capabilities, and closes to the conformer's element at a call: in a
# function's parameter and `ref` result, spelled as a subscript or as
# `__getitem_param__`, in a struct method, and in a trait requirement
# called through the bound.
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


def first_param[
    T: HasTypes
](ref x: TypeList[Trait=AnyType, T._types].__getitem_param__[0]) -> ref[
    x
] TypeList[Trait=AnyType, T._types].__getitem_param__[0]:
    return x


def second[T: HasTypes](x: TypeList[Trait=AnyType, T._types]()[1]) -> Bool:
    return rebind[Bool](x)


struct W:
    def __init__(out self):
        pass

    def first[
        T: HasTypes
    ](self, ref x: TypeList[Trait=AnyType, T._types]()[0]) -> ref[
        x
    ] TypeList[Trait=AnyType, T._types]()[0]:
        return x


# A requirement whose result is an element of the conformer's list.
trait HasGetter:
    comptime _types: _MLIR.KGENParamListType[AnyType]

    def get[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._types]()[id]:
        ...


struct G(HasGetter):
    var a: Int
    comptime _types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, Int, Bool
    ].values

    def __init__(out self):
        self.a = 3

    def get[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._types]()[id]:
        comptime assert id == 0, "only the first"
        return rebind[TypeList[Trait=AnyType, Self._types]()[id]](
            Pointer(to=self.a).unsafe_origin_cast[origin_of(self)]()[]
        )


def first_of[
    T: HasGetter
](ref x: T) -> ref[x] TypeList[Trait=AnyType, T._types]()[0]:
    return x.get[0]()


# The element's capabilities are the member list's trait, whatever `Trait=`
# the construction spells.
trait HasCopyables:
    comptime _types: _MLIR.KGENParamListType[Copyable]


struct C[*Ts: Copyable](HasCopyables):
    comptime _types: _MLIR.KGENParamListType[Copyable] = Self.Ts.values

    def __init__(out self):
        pass


def dup[
    T: HasCopyables
](x: TypeList[Trait=Copyable, T._types]()[0]) -> TypeList[
    Trait=Copyable, T._types
]()[0]:
    return x.copy()


def dup_any[
    T: HasCopyables
](x: TypeList[Trait=AnyType, T._types]()[0]) -> TypeList[
    Trait=AnyType, T._types
]()[0]:
    return x.copy()


def main():
    var n = 3
    print(first[S[Int, Bool]](n))
    print(first_param[S[Int, Bool]](n))
    print(second[S[Int, Bool]](True))
    ref r = first[S[Int, Bool]](n)
    r += 4
    var y: Int = first[S[Int, Bool]](n) + 1
    print(n, y)
    print(W().first[S[Int, Bool]](n))
    var g = G()
    var z: Int = first_of(g)
    print(z + 1)
    print(g.get[0]())
    print(dup[C[Int, Bool]](5))
    print(dup_any[C[Int, Bool]](6))
