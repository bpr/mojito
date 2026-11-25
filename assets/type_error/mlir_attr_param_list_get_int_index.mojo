# A `#kgen.param_list.get` body indexes with an `index` value: an `Int`
# binder spelled straight into the attribute is rejected for its type, as
# the pin rejects it, rather than for the `__mlir_attr` head.
# expect: the index of '#kgen.param_list.get'
from std.builtin.variadics import _MLIR


struct L[Trait: type_of(AnyType), //, values: _MLIR.KGENParamListType[Trait]]:
    comptime _mlir_type = _MLIR.KGENParamListType[Self.Trait]
    comptime get[idx: Int] = __mlir_attr[
        `#kgen.param_list.get<:`,
        Self._mlir_type,
        ` `,
        +Self.values,
        `, `,
        idx,
        `> : `,
        +Self.Trait,
    ]


def main():
    print("unreached")
