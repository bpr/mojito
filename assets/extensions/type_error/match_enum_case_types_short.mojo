# expect: must be a parameter list covering every case
# A case pattern reads its case's payload type, so `_union_case_types`
# must reach that case.
# `UnionLike` is upstream's enum proposal's trait, which the pin does not
# declare (`docs/non-goals.md`).
from std.builtin.variadics import TypeList, _MLIR


struct Open(UnionLike):
    var n: Int

    comptime _sum_case_names = ParameterList.of[
        "A".value, "B".value, "C".value
    ].values
    comptime _union_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, Tuple[], Int
    ].values
    comptime _enum_is_exhaustive = False

    def __init__(out self, n: Int):
        self.n = n

    def _get_sum_discriminant(self) -> Int:
        return 2

    def _unsafe_get_union_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._union_case_types]()[
        id
    ]:
        return rebind[TypeList[Trait=AnyType, Self._union_case_types]()[id]](
            Pointer(to=self.n).unsafe_origin_cast[origin_of(self)]()[]
        )


def main():
    var w = Open(1)
    match w:
        case .C(x):
            print("c", x)
        case _:
            print("other")
