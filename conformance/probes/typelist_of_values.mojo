# PROBE: `TypeList.of[Trait=AnyType, ...].values` as a struct's raw-list `comptime` member (roadmap R1186).
#
# The pin prints:
#   2
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: fails to parse ("only named functions can be called, found Member").
# When Mojito runs it, promote it to `assets/ok/` with its manifest rows.
#
# Run:    mojo run typelist_of_values.mojo
#         cargo run -- run conformance/probes/typelist_of_values.mojo
from std.builtin.variadics import TypeList, _MLIR


struct E:
    comptime types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Int
    ].values


def main():
    print(TypeList[Trait=AnyType, E.types]().length)
