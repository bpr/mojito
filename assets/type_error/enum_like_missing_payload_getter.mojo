# expect: is missing method '_unsafe_get_enum_payload'
# An `EnumLike` conformer must declare the payload getter; the trait has no
# default for it.
from std.builtin.variadics import TypeList, _MLIR


struct Missing(EnumLike):
    comptime _enum_case_names = ParameterList.of["A".value, "B".value].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Int
    ].values

    def __init__(out self):
        pass

    def _get_enum_discriminant(self) -> Int:
        return 0


def main():
    print(Missing()._get_enum_discriminant())
