# Coverage over user `EnumLike` conformers: a repeated case name is one
# constructor per index, so `.Leaf` covers only the first `Leaf` and the
# second is missing; an open conformer (`_enum_is_exhaustive = False`)
# keeps its fall-through after every named case, warns nothing missing,
# and still finds a repeated case.
from std.builtin.variadics import TypeList, _MLIR


struct Rec(EnumLike):
    var tag: Int
    var n: Int

    comptime _enum_case_names = ParameterList.of[
        "Leaf".value, "Node".value, "Leaf".value
    ].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Int
    ].values

    def __init__(out self, tag: Int, n: Int):
        self.tag = tag
        self.n = n

    def _get_enum_discriminant(self) -> Int:
        return self.tag

    def _unsafe_get_enum_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[
        id
    ]:
        return rebind[TypeList[Trait=AnyType, Self._enum_case_types]()[id]](
            Pointer(to=self.n).unsafe_origin_cast[origin_of(self)]()[]
        )


struct Open(EnumLike):
    var tag: Int
    var n: Int

    comptime _enum_case_names = ParameterList.of[
        "A".value, "B".value, "C".value
    ].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Int
    ].values
    comptime _enum_is_exhaustive = False

    def __init__(out self, tag: Int, n: Int):
        self.tag = tag
        self.n = n

    def _get_enum_discriminant(self) -> Int:
        return self.tag

    def _unsafe_get_enum_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[
        id
    ]:
        return rebind[TypeList[Trait=AnyType, Self._enum_case_types]()[id]](
            Pointer(to=self.n).unsafe_origin_cast[origin_of(self)]()[]
        )


def rec_cases(r: Rec):
    match r:
        case .Leaf:
            print("leaf")
        case .Node(x):
            print("node", x)


def open_all_cases(w: Open):
    match w:
        case .A:
            print("a")
        case .B(n):
            print("b", n)
        case .C:
            print("c")
    print("fell through")


def open_repeated_case(w: Open):
    match w:
        case .A:
            print("a")
        case .A:
            print("a again")
        case _:
            print("other")


def main():
    rec_cases(Rec(1, 5))
    rec_cases(Rec(0, 0))
    open_all_cases(Open(1, 3))
    open_all_cases(Open(7, 0))
    open_repeated_case(Open(0, 0))
    open_repeated_case(Open(2, 0))
