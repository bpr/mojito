# Upstream's `VariadicPack` pointer maps, spelled outside the bundled
# library: `#kgen.param.expr<variadic_ptr_map, …>` gives the pointers to a
# pack's types, `variadic_ptrremove_map` their pointees, a
# `!kgen.struct<:param_list<type> L isParamPack>` is the literal struct of
# such a list, and `kgen.struct.load_indirect` loads the struct of pointers
# `get_as_kgen_pack()` rebinds a pack's references to, as the pin's
# `get_loaded_kgen_pack` does.
from std.builtin.variadics import _MLIR


struct P[*Ts: AnyType]:
    comptime _kgen_element_types = rebind[
        _MLIR.KGENParamListType[__mlir_type.`!kgen.type`]
    ](Self.Ts.values)
    comptime _pointers = __mlir_attr[
        `#kgen.param.expr<variadic_ptr_map, `,
        Self._kgen_element_types,
        `, 0: index>: `,
        _MLIR.KGENParamListType[__mlir_type.`!kgen.type`],
    ]
    comptime _pointer_struct = __mlir_type[
        `!kgen.struct<:param_list<type> `, Self._pointers, ` isParamPack>`
    ]
    comptime _removed = __mlir_attr[
        `#kgen.param.expr<variadic_ptrremove_map, `,
        Self._pointers,
        `>: `,
        _MLIR.KGENParamListType[__mlir_type.`!kgen.type`],
    ]


def popcount[*Ts: AnyType](*args: *Ts) -> Int32:
    var loaded = __mlir_op.`kgen.struct.load_indirect`(args.get_as_kgen_pack())
    return __mlir_op.`pop.call_llvm_intrinsic`[
        intrin="llvm.ctpop".value,
        _type=Int32,
        hasSideEffects=False.__mlir_i1__(),
    ](loaded)


def leading_zeros[*Ts: AnyType](*args: *Ts) -> Int64:
    return __mlir_op.`pop.call_llvm_intrinsic`[
        intrin="llvm.ctlz".value,
        _type=Int64,
        hasSideEffects=False.__mlir_i1__(),
    ](args.get_loaded_kgen_pack())


def main():
    print(popcount(Int32(7)))
    print(popcount(Int32(-1)))
    print(leading_zeros(Int64(1), False))
