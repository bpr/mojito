# Enum case patterns over user types declaring `EnumLike`: the case names
# come from `_enum_case_names` (a repeated name selects its first index),
# the payload types from `_enum_case_types`, read only as far as a payload
# pattern needs (a nullary case past the list is fine), and each case reads
# its payload through the getter once, at the first command that needs it.
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
        print("payload", id)
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
        case .Leaf:
            print("leaf")
    print("--")
    match r:
        case .Leaf | .Node(_):
            print("any")
        case _:
            print("other")
    print("--")
    var second_leaf = Rec(2, 0)
    match second_leaf:
        case .Leaf:
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
        case .A:
            print("a")
        case .C:
            print("c")
        case _:
            print("other")
