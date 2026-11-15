# A temporary tuple subject whose case reads a union payload and then tests a
# later element, and a guard reading an `as` binding of a payload followed by
# a read of the place subject after the match: the hidden subject and element
# references name origins and borrow nothing exclusively, so both run.
# `UnionLike` is upstream's enum proposal's trait, which the pin does not
# declare (`docs/non-goals.md`).
from std.builtin.variadics import TypeList, _MLIR


struct Rec(UnionLike, ImplicitlyCopyable):
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
        print("disc")
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


def temp_tuple():
    var r = Rec(1, 5)
    var s = 2
    match (r, s):
        case (.Node(x), 1):
            print("node", x)
        case _:
            print("other")


def guard():
    var r = Rec(1, 5)
    match r:
        case .Node(6 as z) if z > 10:
            print("big", z)
        case _:
            print("other")
    print(r.n)


def main():
    temp_tuple()
    guard()
