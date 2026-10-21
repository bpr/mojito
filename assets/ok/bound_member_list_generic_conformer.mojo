# A raw-list member read through a trait bound, whose conformer is generic
# and spells the list over its own parameter (`Self.T`), folds at the
# instance: its length, and an element as a reference result's type.
from std.builtin.variadics import TypeList, _MLIR


trait HasTypes:
    comptime _types: _MLIR.KGENParamListType[AnyType]

    def first(ref self) -> ref[self] TypeList[Trait=AnyType, Self._types]()[1]:
        ...


struct G[T: Copyable & Deinitable](HasTypes):
    comptime _types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Self.T
    ].values
    var value: Self.T

    def __init__(out self, value: Self.T):
        self.value = value.copy()

    def get(ref self) -> ref[self.value] Self.T:
        return self.value

    def first(ref self) -> ref[self] TypeList[Trait=AnyType, Self._types]()[1]:
        return rebind[TypeList[Trait=AnyType, Self._types]()[1]](
            Pointer(to=self.get()).unsafe_origin_cast[origin_of(self)]()[]
        )


struct H(HasTypes):
    comptime _types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Int, Bool
    ].values
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def first(ref self) -> ref[self] Int:
        return Pointer(to=self.n).unsafe_origin_cast[origin_of(self)]()[]


def length[T: HasTypes]() -> Int:
    return TypeList[Trait=AnyType, T._types]().length


def payload[
    T: HasTypes
](ref x: T) -> ref[x] TypeList[Trait=AnyType, T._types]()[1]:
    return x.first()


def main():
    print(length[H](), length[G[Int]](), length[G[String]]())
    print(TypeList[Trait=AnyType, G[Int]._types]().length)
    var g = G(5)
    var h = H(7)
    ref p = payload(g)
    p += 1
    print(g.value, payload(h))
