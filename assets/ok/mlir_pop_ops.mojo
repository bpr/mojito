# Upstream's `SIMD` operator bodies over a user struct's `!kgen.simd`
# storage: each is one `__mlir_op` `pop` op, as in `std/simd.mojo`.


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

    @staticmethod
    def splat(value: Scalar[Self.dtype]) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.simd.splat`[_type=Self._mlir_type](
                value._mlir_value
            )
        )

    def get(self) -> SIMD[Self.dtype, Self.length]:
        return SIMD[Self.dtype, Self.length](mlir_value=self._mlir_value)

    def __add__(self, rhs: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.add`(self._mlir_value, rhs._mlir_value)
        )

    def __sub__(self, rhs: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.sub`(self._mlir_value, rhs._mlir_value)
        )

    def __mul__(self, rhs: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.mul`(self._mlir_value, rhs._mlir_value)
        )

    def __truediv__(self, rhs: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.div`(self._mlir_value, rhs._mlir_value)
        )

    def __floordiv__(self, rhs: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.floordiv`(
                self._mlir_value, rhs._mlir_value
            )
        )

    def rem(self, rhs: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.rem`(self._mlir_value, rhs._mlir_value)
        )

    def __neg__(self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.neg`(self._mlir_value)
        )

    def __abs__(self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.abs`(self._mlir_value)
        )

    def __lshift__(self, rhs: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.shl`(self._mlir_value, rhs._mlir_value)
        )

    def __rshift__(self, rhs: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.shr`(self._mlir_value, rhs._mlir_value)
        )

    def __and__(self, rhs: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.simd.and`(
                self._mlir_value, rhs._mlir_value
            )
        )

    def __or__(self, rhs: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.simd.or`(self._mlir_value, rhs._mlir_value)
        )

    def __xor__(self, rhs: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.simd.xor`(
                self._mlir_value, rhs._mlir_value
            )
        )

    def lt(self, rhs: Self) -> Lanes[DType.bool, Self.length]:
        return Lanes[DType.bool, Self.length](
            mlir_value=__mlir_op.`pop.cmp`[pred=__mlir_attr.`#kgen.cmp_pred<lt>`](
                self._mlir_value, rhs._mlir_value
            )
        )

    def ne(self, rhs: Self) -> Lanes[DType.bool, Self.length]:
        return Lanes[DType.bool, Self.length](
            mlir_value=__mlir_op.`pop.cmp`[pred=__mlir_attr.`#kgen.cmp_pred<ne>`](
                self._mlir_value, rhs._mlir_value
            )
        )

    def cast[target: DType](self) -> Lanes[target, Self.length]:
        return Lanes[target, Self.length](
            mlir_value=__mlir_op.`pop.cast`[
                _type=Lanes[target, Self.length]._mlir_type,
                fastmathFlags=__mlir_attr.`#pop.fmf<fast>`,
            ](self._mlir_value)
        )

    def fma(self, multiplier: Self, accumulator: Self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.fma`(
                self._mlir_value,
                multiplier._mlir_value,
                accumulator._mlir_value,
            )
        )

    def __round__(self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.round`(self._mlir_value)
        )

    def __floor__(self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.floor`(self._mlir_value)
        )

    def __ceil__(self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.ceil`(self._mlir_value)
        )

    def __trunc__(self) -> Self:
        return Lanes[Self.dtype, Self.length](
            mlir_value=__mlir_op.`pop.trunc`(self._mlir_value)
        )


def main():
    var i = Lanes[DType.int32, 4](
        mlir_value=SIMD[DType.int32, 4](-7, 7, -8, 9)._mlir_value
    )
    var j = Lanes[DType.int32, 4].splat(2)
    print((i + j).get(), (i - j).get(), (i * j).get())
    print((i / j).get(), (i // j).get(), i.rem(j).get())
    print((-i).get(), i.__abs__().get(), (i << j).get(), (i >> j).get())
    print((i & j).get(), (i | j).get(), (i ^ j).get())
    print(i.lt(j).get(), i.ne(j).get())
    print(i.cast[DType.float32]().get())
    var b = Lanes[DType.bool, 4](
        mlir_value=SIMD[DType.bool, 4](True, False, True, False)._mlir_value
    )
    print((b ^ i.lt(j)).get())
    var f = Lanes[DType.float32, 4](
        mlir_value=SIMD[DType.float32, 4](2.5, -2.5, 7.5, -7.5)._mlir_value
    )
    var g = Lanes[DType.float32, 4].splat(2.0)
    print((f / g).get(), (f // g).get(), f.rem(g).get())
    print(
        f.__round__().get(),
        f.__floor__().get(),
        f.__ceil__().get(),
        f.__trunc__().get(),
    )
    print(f.fma(g, g).get(), f.__abs__().get(), f.cast[DType.int32]().get())
    var n = Lanes[DType.int, 1](mlir_value=SIMD[DType.int, 1](-7)._mlir_value)
    var m = Lanes[DType.int, 1](mlir_value=SIMD[DType.int, 1](2)._mlir_value)
    print((n / m).get(), (n // m).get(), n.rem(m).get(), n.lt(m).get())
    var u = Lanes[DType.uint8, 2](
        mlir_value=SIMD[DType.uint8, 2](250, 7)._mlir_value
    )
    var v = Lanes[DType.uint8, 2](mlir_value=SIMD[DType.uint8, 2](3, 2)._mlir_value)
    print((u / v).get(), u.rem(v).get(), (u >> v).get(), u.lt(v).get())
    var p = Lanes[DType.float64, 1](mlir_value=Float64(-7.5)._mlir_value)
    var q = Lanes[DType.float64, 1].splat(2.0)
    print(p.rem(q).get(), (p // q).get(), p.__round__().get(), p.fma(q, q).get())
    print(p.cast[DType.int]().get(), n.cast[DType.float64]().get())
    print(f.get() // g.get(), Float32(7.5) // Float32(-2.0))
    print(
        SIMD[DType.float64, 4](
            mlir_value=__mlir_op.`pop.cast`[_type=SIMD[DType.float64, 4]._mlir_type](
                i._mlir_value
            )
        )
    )
