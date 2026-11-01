# `SIMD(mlir_value=s)` infers `dtype` and `length` from storage spelled over
# binders, as upstream's `cast` and `select` build it
# (`assets/type_error/simd_mlir_value_int_width.mojo` is a width an `Int`
# binder converts to).
def double[dt: DType, n: SIMDLength](v: SIMD[dt, n]) -> SIMD[dt, n]:
    return SIMD(mlir_value=__mlir_op.`pop.add`(v._mlir_value, v._mlir_value))


def widen[
    dt: DType, n: SIMDLength, //, target: DType
](v: SIMD[dt, n]) -> SIMD[target, n]:
    return SIMD(
        mlir_value=__mlir_op.`pop.cast`[_type=SIMD[target, n]._mlir_type](
            v._mlir_value
        )
    )


def main():
    print(double(SIMD[DType.int32, 4](1, 2, 3, 4)))
    print(double(Float64(1.25)))
    print(widen[DType.float32](SIMD[DType.int16, 2](-3, 4)))
    var s = SIMD[DType.uint8, 2](11, 12)
    print(SIMD[DType.uint8, 2](mlir_value=s._mlir_value))
