# A struct `comptime` member whose body is still open over the struct's
# parameters checks its body's type against its annotation at the
# declaration: a KGEN `index` attribute is not an `Int`.
# expect: type mismatch for comptime member 'n': expected Int, found __mlir_type.index
from std.builtin.variadics import _MLIR


struct L[Trait: type_of(AnyType), //, values: _MLIR.KGENParamListType[Trait]]:
    comptime _mlir_type = _MLIR.KGENParamListType[Self.Trait]
    comptime n: Int = __mlir_attr[
        `#kgen.param_list.size<:`,
        Self._mlir_type,
        ` `,
        +Self.values,
        `> : index`,
    ]


def show[*Ts: AnyType]():
    print(L[Ts.values].n)


def main():
    show[Int, Bool]()
