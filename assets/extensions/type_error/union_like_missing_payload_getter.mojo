# expect: is missing method '_unsafe_get_union_payload'
# A `UnionLike` conformer must declare the payload getter; the trait has no
# default for it.
# `UnionLike` is upstream's enum proposal's trait, which the pin does not
# declare (`docs/non-goals.md`).
from std.builtin.variadics import TypeList, _MLIR


struct Missing(UnionLike):
    comptime _sum_case_names = ParameterList.of["A".value, "B".value].values
    comptime _union_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, Tuple[], Int
    ].values

    def __init__(out self):
        pass

    def _get_sum_discriminant(self) -> Int:
        return 0


def main():
    print(Missing()._get_sum_discriminant())
