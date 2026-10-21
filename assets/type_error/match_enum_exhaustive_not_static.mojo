# An `EnumLike` whose `_enum_is_exhaustive` is a parameter cannot be
# matched in a body where the parameter is unknown.
# expect: '_enum_is_exhaustive' must be statically known when matching an EnumLike type
from std.builtin.variadics import TypeList, _MLIR


struct E[flag: Bool](EnumLike):
    var tag: Int

    comptime _enum_case_names = ParameterList.of["A".value, "B".value].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, NoneType
    ].values
    comptime _enum_is_exhaustive = Self.flag

    def __init__(out self, tag: Int):
        self.tag = tag

    def _get_enum_discriminant(self) -> Int:
        return self.tag

    def _unsafe_get_enum_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[
        id
    ]:
        return rebind[TypeList[Trait=AnyType, Self._enum_case_types]()[id]](
            Pointer(to=self.tag).unsafe_origin_cast[origin_of(self)]()[]
        )


def f[b: Bool](e: E[b]):
    match e:
        case .A:
            print("a")
        case _:
            print("other")


def main():
    f(E[True](0))
