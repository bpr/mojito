# Upstream's `std/sys/intrinsics.mojo` surface for the proof subset:
# `llvm_intrinsic`, the call of an LLVM intrinsic by name through
# `pop.call_llvm_intrinsic`. The intrinsics Mojito computes are
# `mojito_types::llvm_intrinsic::LlvmIntrinsic`'s; a reached call of any
# other is rejected when its instance is elaborated.
#
# The body is upstream's, except:
# - `intrin` is a `StringLiteral`, where upstream's is a `StaticString`,
#   which Mojito does not declare (roadmap R432);
# - its `!kgen.string` is `intrin.value`, where upstream's is
#   `_get_kgen_string[intrin]()` over the `StaticString`;
# - `args.get_loaded_kgen_pack()` is served by the compiler on the
#   collector, which is compiler storage rather than upstream's nominal
#   `VariadicPack` (roadmap R1302).
# `_RegisterPackType` is upstream's, verbatim: its `!kgen.struct` field is
# Mojito's compiler-private tuple storage of the same elements.
# Omitted: the gather, scatter, masked load and store, prefetch,
# `assume`/`expect`, and GPU intrinsics.
"""Defines intrinsics.

You can import these APIs from the `sys` package. For example:

```mojo
from std.sys import llvm_intrinsic
```
"""


@inline(.nodebug)
def llvm_intrinsic[
    intrin: StringLiteral,
    type: TrivialRegisterPassable,
    *types: AnyType,
    has_side_effect: Bool = True,
](*args: *types) -> type:
    """Calls an LLVM intrinsic with the name `intrin` and return type `type`.

    Parameters:
        intrin: The name of the llvm intrinsic.
        type: The return type of the intrinsic.
        types: The argument types for the function.
        has_side_effect: If `True` the intrinsic will have side effects,
            otherwise its pure.

    Args:
        args: The arguments to the function.

    Returns:
        The result of calling the llvm intrinsic with no arguments.
    """

    var loaded_pack = args.get_loaded_kgen_pack()

    comptime intrin_kgen_string = intrin.value

    comptime if type == NoneType:
        __mlir_op.`pop.call_llvm_intrinsic`[
            intrin=intrin_kgen_string,
            _type=None,
            hasSideEffects=has_side_effect.__mlir_i1__(),
        ](loaded_pack)
        return rebind[type](None)

    else:
        return __mlir_op.`pop.call_llvm_intrinsic`[
            intrin=intrin_kgen_string,
            _type=type,
            hasSideEffects=has_side_effect.__mlir_i1__(),
        ](loaded_pack)


# ===----------------------------------------------------------------------=== #
# Transitional type used for llvm_intrinsic
# ===----------------------------------------------------------------------=== #


struct _RegisterPackType[*Ts: TrivialRegisterPassable](TrivialRegisterPassable):
    comptime _mlir_type = __mlir_type[
        `!kgen.struct<`, ~Self.Ts.values, ` isParamPack>`
    ]

    var _mlir_value: Self._mlir_type

    @inline(.nodebug)
    def __getitem_param__[i: Int](self) -> Self.Ts[i]:
        """Get the element.

        Parameters:
            i: The element index.

        Returns:
            The tuple element at the requested index.
        """
        return __mlir_op.`kgen.struct.extract`[index=i.__mlir_index__()](
            self._mlir_value
        )
