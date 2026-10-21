# expect: must be a parameter list covering every case
# A payload pattern reads its case's payload type, so `_enum_case_types`
# must reach that case; a nullary `.C` alone would not need it.
from std.builtin.variadics import TypeList, _MLIR


struct Open(EnumLike):
    var n: Int

    comptime _enum_case_names = ParameterList.of[
        "A".value, "B".value, "C".value
    ].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Int
    ].values
    comptime _enum_is_exhaustive = False

    def __init__(out self, n: Int):
        self.n = n

    def _get_enum_discriminant(self) -> Int:
        return 2

    def _unsafe_get_enum_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[
        id
    ]:
        return rebind[TypeList[Trait=AnyType, Self._enum_case_types]()[id]](
            Pointer(to=self.n).unsafe_origin_cast[origin_of(self)]()[]
        )


def main():
    var w = Open(1)
    match w:
        case .C(x):
            print("c", x)
        case _:
            print("other")
