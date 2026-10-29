# `#kgen.param_list.size` and `#kgen.param_list.get` over a raw parameter
# list, spelled with upstream's bracketed `__mlir_attr`, are the list's
# length as an `index` and its element at one; a `+` before an operand is
# KGEN's splice. `TypeList`'s `_raw_length`, `length`, and
# `_get_type_at_index`, and `ParameterList`'s `size`, are those bodies.
from std.builtin.variadics import _MLIR, TypeList


struct L[Trait: type_of(AnyType), //, values: _MLIR.KGENParamListType[Trait]]:
    comptime _mlir_type = _MLIR.KGENParamListType[Self.Trait]
    comptime _raw_length: __mlir_type.index = __mlir_attr[
        `#kgen.param_list.size<:`,
        Self._mlir_type,
        ` `,
        +Self.values,
        `> : index`,
    ]
    comptime length = Int(mlir_value=Self._raw_length)
    comptime _get_type_at_index[idx: __mlir_type.index] = __mlir_attr[
        `#kgen.param_list.get<:`,
        Self._mlir_type,
        ` `,
        +Self.values,
        `, `,
        idx,
        `> : `,
        +Self.Trait,
    ]


def show[*Ts: AnyType]():
    comptime l = L[Ts.values]
    print(l.length)
    comptime T1 = l._get_type_at_index[__mlir_attr.`1 : index`]
    comptime assert T1 == Int
    print("T1 is Int")
    comptime n = l._raw_length
    print(Int(mlir_value=n))
    comptime t = TypeList[Ts.values]
    comptime assert t._get_type_at_index[__mlir_attr.`2 : index`] == Bool
    print(Int(mlir_value=t._raw_length), t.length, Ts.length)


struct H[*Ts: AnyType]:
    comptime n = TypeList[Self.Ts.values].length
    comptime raw = TypeList[Self.Ts.values]._raw_length


def count[*vs: Int]() -> Int:
    return ParameterList[vs.values].size


def main():
    show[String, Int, Bool]()
    comptime tl = TypeList.of[Trait=AnyType, Int, String, Float64]
    print(Int(mlir_value=tl._raw_length))
    comptime T = tl._get_type_at_index[__mlir_attr.`2 : index`]
    print(T(1.5))
    print(H[Int, Int].n, Int(mlir_value=H[Int].raw), count[4, 5, 6]())
