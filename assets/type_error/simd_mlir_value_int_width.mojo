# expect: cannot infer type parameter 'length'
# `SIMD`'s `mlir_value=` initializer infers `length` only from storage a
# `SIMDLength` binder spells; an `Int` binder's width is its conversion,
# which binds nothing, as at the pin.
def double_int[dt: DType, n: Int](v: SIMD[dt, n]) -> SIMD[dt, n]:
    var r = __mlir_op.`pop.add`(v._mlir_value, v._mlir_value)
    return SIMD(mlir_value=r)


def main():
    print(double_int[DType.int32, 4](SIMD[DType.int32, 4](5, 6, 7, 8)))
