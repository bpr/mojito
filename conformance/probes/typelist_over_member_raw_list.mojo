# PROBE: `TypeList[Trait=AnyType, ...]()` over a raw list a struct's `comptime` member holds, read concretely and through a trait bound (roadmap R1184).
#
# The pin prints:
#   3
#   2
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: stops with "TypeList[...] takes a pack projection ('Ts.values')".
# When Mojito runs it, promote it to `assets/ok/` with its manifest rows.
#
# Run:    mojo run typelist_over_member_raw_list.mojo
#         cargo run -- run conformance/probes/typelist_over_member_raw_list.mojo
from std.builtin.variadics import TypeList, _MLIR


trait HasTypes:
    comptime _types: _MLIR.KGENParamListType[AnyType]


struct S[*Ts: AnyType](HasTypes):
    comptime _types: _MLIR.KGENParamListType[AnyType] = Self.Ts.values

    def __init__(out self):
        pass


def count[T: HasTypes]() -> Int:
    return TypeList[Trait=AnyType, T._types]().length


def main():
    print(count[S[Int, Bool, NoneType]]())
    print(TypeList[Trait=AnyType, S[Int, Bool]._types]().length)
