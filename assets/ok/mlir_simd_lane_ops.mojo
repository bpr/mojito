# Upstream's `SIMD` lane bodies over a user struct's `!kgen.simd` storage:
# `__getitem__`, `__setitem__`, `select`, `__eq__`, and `__bool__` are each
# one `pop.simd` op, as in `std/simd.mojo`. The compiler's own `SIMD`
# methods and subscripts compute the same ops.


struct Lanes[dtype: DType, length: SIMDLength](Copyable):
    comptime _mlir_type = __mlir_type[
        `!kgen.simd<`,
        Self.length._mlir_value,
        `, `,
        Self.dtype._mlir_value,
        `>`,
    ]

    var _mlir_value: Self._mlir_type

    def __init__(out self, *, mlir_value: Self._mlir_type):
        self._mlir_value = mlir_value

    def get(self) -> SIMD[Self.dtype, Self.length]:
        return SIMD[Self.dtype, Self.length](mlir_value=self._mlir_value)

    def __getitem__(self, idx: Int) -> Scalar[Self.dtype]:
        return Scalar[Self.dtype](
            mlir_value=__mlir_op.`pop.simd.extractelement`(
                self._mlir_value, idx.__mlir_index__()
            )
        )

    def __setitem__(mut self, idx: Int, val: Scalar[Self.dtype]):
        self._mlir_value = __mlir_op.`pop.simd.insertelement`(
            self._mlir_value, val._mlir_value, idx.__mlir_index__()
        )

    def bits_and(self) -> Scalar[Self.dtype]:
        return Scalar[Self.dtype](
            mlir_value=__mlir_op.`pop.simd.reduce_and`(self._mlir_value)
        )

    def bits_or(self) -> Scalar[Self.dtype]:
        return Scalar[Self.dtype](
            mlir_value=__mlir_op.`pop.simd.reduce_or`(self._mlir_value)
        )


struct Mask[length: SIMDLength](Copyable):
    comptime _mlir_type = __mlir_type[
        `!kgen.simd<`,
        Self.length._mlir_value,
        `, `,
        DType.bool._mlir_value,
        `>`,
    ]

    var _mlir_value: Self._mlir_type

    def __init__(out self, value: SIMD[DType.bool, Self.length]):
        self._mlir_value = value._mlir_value

    def all_set(self) -> Bool:
        return Bool(mlir_value=__mlir_op.`pop.simd.reduce_and`(self._mlir_value))

    def any_set(self) -> Bool:
        return Bool(mlir_value=__mlir_op.`pop.simd.reduce_or`(self._mlir_value))

    def select[
        T: DType
    ](self, true_case: Lanes[T, Self.length], false_case: Lanes[T, Self.length]) -> Lanes[
        T, Self.length
    ]:
        return Lanes[T, Self.length](
            mlir_value=__mlir_op.`pop.simd.select`(
                self._mlir_value, true_case._mlir_value, false_case._mlir_value
            )
        )


def main():
    var v = Lanes[DType.int32, 4](
        mlir_value=SIMD[DType.int32, 4](1, 2, 3, 4)._mlir_value
    )
    print(v[0], v[3])
    v[1] = 20
    print(v.get())
    var mask = Mask[4](v.get().gt(2))
    print(mask.all_set(), mask.any_set())
    var none = Mask[4](SIMD[DType.bool, 4](fill=False))
    print(none.all_set(), none.any_set())
    var floats = Lanes[DType.float32, 4](
        mlir_value=SIMD[DType.float32, 4](0.5, 1.5, 2.5, 3.5)._mlir_value
    )
    var fallback = Lanes[DType.float32, 4](
        mlir_value=SIMD[DType.float32, 4](-1.0)._mlir_value
    )
    print(mask.select(floats, fallback).get())
    var bits = Lanes[DType.uint8, 2](
        mlir_value=SIMD[DType.uint8, 2](0xF0, 0x3C)._mlir_value
    )
    print(bits.bits_and(), bits.bits_or())

    # The compiler's `SIMD` lane reads and writes, plain, augmented, and
    # through a field, and its `select` and reductions.
    var w = SIMD[DType.int64, 4](10, 20, 30, 40)
    w[0] = 11
    w[3] += 5
    var k = 2
    print(w, w[k], w[k - 1])
    var m = w.gt(15)
    print(m.select(w, SIMD[DType.int64, 4](0)), m.select(w, 7), m.select(Int64(1), w))
    print(m.reduce_and(), m.reduce_or(), w.reduce_and(), w.reduce_or())
    if m.reduce_or() and not m.reduce_and():
        print("mixed")
    var one = SIMD[DType.bool, 1](True)
    print(one.select(Int32(4), Int32(5)), one.reduce_and())
    var holder = Lanes[DType.int32, 4](
        mlir_value=SIMD[DType.int32, 4](0)._mlir_value
    )
    holder[2] = Int32(9)
    print(holder.get())
