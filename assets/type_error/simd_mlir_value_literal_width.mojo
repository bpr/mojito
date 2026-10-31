# expect: cannot infer type parameter 'length'
# A literal width in a generic body's storage binds no `length`, though its
# `dtype` binder binds `dtype`.
def two[dt: DType](v: SIMD[dt, 2]) -> SIMD[dt, 2]:
    return SIMD(mlir_value=__mlir_op.`pop.add`(v._mlir_value, v._mlir_value))


def main():
    print(two(SIMD[DType.int32, 2](1, 2)))
