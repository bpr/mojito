# `ParameterList.of` and `empty_of` are the bundled struct's upstream
# aliases: `of[type: AnyType, //, *values: type]` names
# `ParameterList[type=type, values.values]`, its `type` inferred from the
# values or given by keyword, and `empty_of[type]` is `Self.of[type=type]`,
# declared above it. A read through either is a type (`Ints.size`), a
# construction through either is a `ParameterList` value, and a closed
# list's `.values` is a raw list a module or local `comptime` binds and a
# construction takes.
from std.builtin.variadics import _MLIR

comptime KGENString = __mlir_type.`!kgen.string`
comptime pair = ParameterList.of[1, 2].values


def main():
    comptime Ints = ParameterList.of[4, 5, 6]
    print(Ints.size, len(Ints()))
    print(Ints())
    var total = 0
    for v in ParameterList.of[4, 5, 6]():
        total += v
    print(total)
    print(ParameterList.of[1, 2]()[1])
    comptime E = ParameterList.empty_of[Int]
    print(E.size, len(E()))
    print(ParameterList.empty_of[Int]())
    print(ParameterList.of[type=Int8, 1, 2]())
    print(ParameterList.of[type=Float32, 1.5, 2.25](), ParameterList.of[type=UInt16, 300, 7]())
    print(ParameterList[pair](), ParameterList[pair].size)
    comptime raw: _MLIR.KGENParamListType[Int] = ParameterList.of[7, 8].values
    print(ParameterList[raw].size, ParameterList[raw]())
    comptime names: _MLIR.KGENParamListType[KGENString] = ParameterList.of[
        "None".value, "Some".value
    ].values
    print(ParameterList[names]().size)
