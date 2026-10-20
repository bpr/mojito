# expect: value passed to '_types' cannot be converted from 'KGENParamList[AnyType]' to 'KGENParamList[Copyable]'
# A raw-list member's body converts to its declared list at the declaration:
# an `AnyType` pack's values are no list of `Copyable` types.
from std.builtin.variadics import TypeList, _MLIR


struct S[*Ts: AnyType]:
    comptime _types: _MLIR.KGENParamListType[Copyable] = Self.Ts.values


def main():
    print(TypeList[S[Int, Bool]._types]().length)
