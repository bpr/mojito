# PROBE: a subscript of a `TypeList` value (`TypeList[...]()[1]`) as a parameter type, a result type, and a local annotation (roadmap R1186).
#
# The pin prints:
#   3 4
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: the signature runs since R1185; the closed annotation stops parsing at `TypeList.of[...].values` ("only named functions can be called"), which needs `TypeList.of` (R1186).
# When Mojito runs it, promote it to `assets/ok/` with its manifest rows.
#
# Run:    mojo run typelist_value_subscript_type.mojo
#         cargo run -- run conformance/probes/typelist_value_subscript_type.mojo
from std.builtin.variadics import TypeList


def second[
    *Ts: AnyType
](ref x: TypeList[Trait=AnyType, Ts.values]()[1]) -> ref[x] TypeList[
    Trait=AnyType, Ts.values
]()[1]:
    return x


def main():
    var n = 3
    var y: TypeList[
        Trait=AnyType, TypeList.of[Trait=AnyType, Bool, Int].values
    ]()[1] = 4
    print(second[Bool, Int](n), y)
