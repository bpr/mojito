# expect: DType ui128 is not supported yet
# A `#kgen.dtype.constant` naming a dtype Mojito has no representation for
# rejects explicitly, as its `DType` member does (R65); the pin runs it.
def main():
    comptime a = __mlir_attr.`#kgen.dtype.constant<ui128> : !kgen.dtype`
    var x: __mlir_type.`!kgen.dtype` = a
    print(
        UInt8(
            mlir_value=__mlir_op.`pop.cast_from_builtin`[
                _type=__mlir_type.`!kgen.scalar<ui8>`
            ](__mlir_op.`pop.dtype.to_ui8`(x))
        )
    )
