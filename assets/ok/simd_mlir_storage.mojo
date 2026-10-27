# Upstream's `SIMD` storage spelling: a struct field typed
# `__mlir_type[`!kgen.simd<`, length._mlir_value, `, `, dtype._mlir_value, `>`]`,
# filled from and read back into a vector through `_mlir_value`.


struct Lanes[dtype: DType, length: SIMDLength](Copyable):
    comptime _mlir_type = __mlir_type[
        `!kgen.simd<`,
        Self.length._mlir_value,
        `, `,
        Self.dtype._mlir_value,
        `>`,
    ]

    var _mlir_value: Self._mlir_type

    def __init__(out self, value: SIMD[Self.dtype, Self.length]):
        self._mlir_value = value._mlir_value

    def get(self) -> SIMD[Self.dtype, Self.length]:
        return SIMD[Self.dtype, Self.length](mlir_value=self._mlir_value)

    def set(mut self, value: SIMD[Self.dtype, Self.length]):
        self._mlir_value = value._mlir_value


def doubled[
    dt: DType, n: SIMDLength
](storage: __mlir_type[`!kgen.simd<`, n._mlir_value, `, `, dt._mlir_value, `>`]) -> SIMD[dt, n]:
    var v = SIMD[dt, n](mlir_value=storage)
    return v + v


def main():
    var lanes = Lanes[DType.int32, 4](SIMD[DType.int32, 4](1, 2, 3, 4))
    print(lanes.get())
    lanes.set(SIMD[DType.int32, 4](5, 6, 7, 8))
    print(lanes.get())
    var halves = SIMD[DType.float32, 2](1.5, 2.5)
    print(doubled[DType.float32, 2](halves._mlir_value))
    var n = 21
    print(Int(mlir_value=n._mlir_value) * 2)
    var copy = lanes.copy()
    print(copy.get()[2])
    var w = SIMD[DType.int32, 2](1, 2)
    w._mlir_value = SIMD[DType.int32, 2](3, 4)._mlir_value
    print(w, SIMD[DType.int32, 2](mlir_value=(w + w)._mlir_value))
