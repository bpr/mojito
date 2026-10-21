# PROBE: consecutive enum cases sharing a leading tag test read the discriminant once per cluster; a `|` reads it per alternative.
#
# The pin runs it and prints:
#   disc
#   payload 1
#   payload 1
#   payload 1
#   node 6
#   --
#   disc
#   disc
#   any
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: prints `disc` before every `.Node` case (three times) until R1237
# clusters cases.
#
# Run:    mojo run match_enum_case_cluster.mojo
#         cargo run -- run conformance/probes/match_enum_case_cluster.mojo
from std.builtin.variadics import TypeList, _MLIR


struct Rec(EnumLike):
    var tag: Int
    var n: Int

    comptime _enum_case_names = ParameterList.of["Leaf".value, "Node".value].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Int
    ].values

    def __init__(out self, tag: Int, n: Int):
        self.tag = tag
        self.n = n

    def _get_enum_discriminant(self) -> Int:
        print("disc")
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
