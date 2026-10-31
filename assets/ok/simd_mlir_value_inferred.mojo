# `SIMD(mlir_value=s)` infers `dtype` and `length` from storage spelled over
# binders, as upstream's `cast` and `select` build it.
def double[dt: DType, n: SIMDLength](v: SIMD[dt, n]) -> SIMD[dt, n]:
    return SIMD(mlir_value=__mlir_op.`pop.add`(v._mlir_value, v._mlir_value))


def double_int[dt: DType, n: Int](v: SIMD[dt, n]) -> SIMD[dt, n]:
    var r = __mlir_op.`pop.add`(v._mlir_value, v._mlir_value)
    comptime if n > 2:
        print("wide")
    return SIMD(mlir_value=r)


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
    print(double_int[DType.int32, 4](SIMD[DType.int32, 4](5, 6, 7, 8)))
    print(double_int[DType.int8, 2](SIMD[DType.int8, 2](9, 10)))
    print(widen[DType.float32](SIMD[DType.int16, 2](-3, 4)))
    var s = SIMD[DType.uint8, 2](11, 12)
    print(SIMD[DType.uint8, 2](mlir_value=s._mlir_value))
