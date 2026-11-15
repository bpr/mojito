# Case patterns over user types declaring `UnionLike`: the case names
# come from `_sum_case_names` (a repeated name selects its first index),
# the payload types from `_union_case_types`, and each case reads its
# payload through the getter once, at the first command that needs it; an
# empty payload pattern reads nothing.
# `UnionLike` is upstream's enum proposal's trait, which the pin does not
# declare (`docs/non-goals.md`).
from std.builtin.variadics import TypeList, _MLIR


struct Rec(UnionLike):
    var tag: Int
    var n: Int

    comptime _sum_case_names = ParameterList.of[
        "Leaf".value, "Node".value, "Leaf".value
    ].values
    comptime _union_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, Tuple[], Int, Tuple[]
    ].values

    def __init__(out self, tag: Int, n: Int):
        self.tag = tag
        self.n = n

    def _get_sum_discriminant(self) -> Int:
        return self.tag

    def _unsafe_get_union_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._union_case_types]()[
        id
    ]:
        print("payload", id)
        return rebind[TypeList[Trait=AnyType, Self._union_case_types]()[id]](
            Pointer(to=self.n).unsafe_origin_cast[origin_of(self)]()[]
        )


struct Open(UnionLike):
    var tag: Int
    var n: Int

    comptime _sum_case_names = ParameterList.of[
        "A".value, "B".value, "C".value
    ].values
    comptime _union_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, Tuple[], Int, Tuple[]
    ].values
    comptime _enum_is_exhaustive = False

    def __init__(out self, tag: Int, n: Int):
        self.tag = tag
        self.n = n

    def _get_sum_discriminant(self) -> Int:
        return self.tag

    def _unsafe_get_union_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._union_case_types]()[
        id
    ]:
        return rebind[TypeList[Trait=AnyType, Self._union_case_types]()[id]](
            Pointer(to=self.n).unsafe_origin_cast[origin_of(self)]()[]
        )


def main():
    var r = Rec(1, 5)
    match r:
        case .Node(0):
            print("zero")
        case .Node(x) if x > 100:
            print("big")
        case Rec.Node(var x):
            x += 1
            print("node", x)
        case .Leaf():
            print("leaf")
    print("--")
    match r:
        case .Leaf() | .Node(_):
            print("any")
        case _:
            print("other")
    print("--")
    var second_leaf = Rec(2, 0)
    match second_leaf:
        case .Leaf():
            print("leaf")
        case _:
            print("other")
    print("--")
    match r:
        case .Node(ref m):
            m = 42
        case _:
            pass
    print(r.n)
    print("--")
    var w = Open(2, 0)
    match w:
        case .A():
            print("a")
        case .C():
            print("c")
        case _:
            print("other")
