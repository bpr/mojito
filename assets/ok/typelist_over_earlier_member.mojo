# A `TypeList` over a struct's earlier raw-list member, as at the pin: the
# member passes as the list's `values` argument in a later member's body, a
# parameterized member's body, a method body, and a method signature, read
# through `Self` in a trait's default method and requirement, which a
# conformer's witness matches. An unannotated member over `Self.Ts.values`
# or `TypeList.of[...].values` keeps its raw-list type.
from std.builtin.variadics import TypeList, _MLIR


trait HasTypes:
    comptime _types: _MLIR.KGENParamListType[ImplicitlyCopyable]

    def first(self) -> TypeList[Trait=ImplicitlyCopyable, Self._types]()[0]:
        ...

    def count(self) -> Int:
        return TypeList[Trait=ImplicitlyCopyable, Self._types]().length


struct S[*Ts: ImplicitlyCopyable](HasTypes):
    comptime _types: _MLIR.KGENParamListType[
        ImplicitlyCopyable
    ] = Self.Ts.values
    comptime n = TypeList[Self._types].length
    var x: Int

    def __init__(out self):
        self.x = 7

    def first(self) -> TypeList[Trait=ImplicitlyCopyable, Self._types]()[0]:
        return rebind[TypeList[Trait=ImplicitlyCopyable, Self._types]()[0]](
            self.x
        )

    def get[
        id: Int
    ](self) -> TypeList[
        Trait=ImplicitlyCopyable, Self._types
    ].__getitem_param__[id]:
        return rebind[
            TypeList[Trait=ImplicitlyCopyable, Self._types].__getitem_param__[
                id
            ]
        ](self.x)

    def size(self) -> Int:
        return TypeList[Trait=ImplicitlyCopyable, Self._types]().length


struct O[*Ts: AnyType]:
    comptime _cases = Self.Ts.values
    comptime E[id: Int]: AnyType = TypeList[Trait=AnyType, Self._cases]()[id]


struct N[T: AnyType]:
    comptime _cases = TypeList.of[Trait=AnyType, NoneType, Self.T].values
    comptime n = TypeList[Self._cases].length


def show[T: HasTypes](t: T):
    print(TypeList[Trait=ImplicitlyCopyable, T._types]().length)


def main():
    var s = S[Int, Bool]()
    print(S[Int, Bool].n, s.first(), s.get[0](), s.size(), s.count())
    show(s)
    var x: O[Bool, String].E[1] = "s"
    var y: TypeList[Trait=AnyType, O[Bool, String]._cases]()[1] = "t"
    print(x, y, TypeList[O[Int, String]._cases].length)
    print(N[String].n, TypeList[N[Int]._cases].length)
