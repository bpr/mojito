# `pop.simd.shuffle` gathers the lanes of its first operand followed by those
# of its second, by a `#pop.array` mask of indices, into the `_type` vector:
# the op upstream's `SIMD.shuffle` and `join` bodies reach.


def gather[
    n: Int
](x: SIMD[DType.int32, n]) -> SIMD[DType.int32, 2]:
    return SIMD[DType.int32, 2](
        mlir_value=__mlir_op.`pop.simd.shuffle`[
            mask=__mlir_attr.`#pop.array<5, 0> : !pop.array<2, index>`,
            _type=SIMD[DType.int32, 2]._mlir_type,
        ](x._mlir_value, x._mlir_value)
    )


def swapped[
    n: Int
](x: SIMD[DType.int32, n]) -> SIMD[DType.int32, n]:
    return SIMD[DType.int32, n](
        mlir_value=__mlir_op.`pop.simd.shuffle`[
            mask=__mlir_attr.`#pop.array<1, 0> : !pop.array<2, index>`,
            _type=SIMD[DType.int32, n]._mlir_type,
        ](x._mlir_value, x._mlir_value)
    )


def reversed4() -> SIMD[DType.int32, 4]:
    var a = SIMD[DType.int32, 4](1, 2, 3, 4)
    return SIMD[DType.int32, 4](
        mlir_value=__mlir_op.`pop.simd.shuffle`[
            mask=__mlir_attr.`#pop.array<3, 2, 1, 0> : !pop.array<4, index>`,
            _type=SIMD[DType.int32, 4]._mlir_type,
        ](a._mlir_value, a._mlir_value)
    )


def main():
    var a = SIMD[DType.int32, 4](1, 2, 3, 4)
    var b = SIMD[DType.int32, 4](5, 6, 7, 8)
    var permuted = __mlir_op.`pop.simd.shuffle`[
        mask=__mlir_attr.`#pop.array<7, 2, 4, 0> : !pop.array<4, index>`,
        _type=SIMD[DType.int32, 4]._mlir_type,
    ](a._mlir_value, b._mlir_value)
    print(SIMD[DType.int32, 4](mlir_value=permuted))
    var narrowed = __mlir_op.`pop.simd.shuffle`[
        mask=__mlir_attr.`#pop.array<1, 0> : !pop.array<2, index>`,
        _type=SIMD[DType.int32, 2]._mlir_type,
    ](a._mlir_value, b._mlir_value)
    print(SIMD[DType.int32, 2](mlir_value=narrowed))
    var interleaved = __mlir_op.`pop.simd.shuffle`[
        mask=__mlir_attr.`#pop.array<0, 4, 1, 5, 2, 6, 3, 7> : !pop.array<8, index>`,
        _type=SIMD[DType.int32, 8]._mlir_type,
    ](a._mlir_value, b._mlir_value)
    print(SIMD[DType.int32, 8](mlir_value=interleaved))
    var t = SIMD[DType.bool, 4](True, False, False, True)
    var u = SIMD[DType.bool, 4](False, False, True, True)
    var flags = __mlir_op.`pop.simd.shuffle`[
        mask=__mlir_attr.`#pop.array<1, 6, 0, 7> : !pop.array<4, index>`,
        _type=SIMD[DType.bool, 4]._mlir_type,
    ](t._mlir_value, u._mlir_value)
    print(SIMD[DType.bool, 4](mlir_value=flags))
    var f = SIMD[DType.float64, 2](1.5, 2.5)
    var widened = __mlir_op.`pop.simd.shuffle`[
        mask=__mlir_attr.`#pop.array<3, 2, 1, 0> : !pop.array<4, index>`,
        _type=SIMD[DType.float64, 4]._mlir_type,
    ](f._mlir_value, (f * 2)._mlir_value)
    print(SIMD[DType.float64, 4](mlir_value=widened))
    comptime mask = __mlir_attr.`#pop.array<2, 3> : !pop.array<2, index>`
    var bound = __mlir_op.`pop.simd.shuffle`[
        mask=mask, _type=SIMD[DType.int32, 2]._mlir_type
    ](a._mlir_value, b._mlir_value)
    print(SIMD[DType.int32, 2](mlir_value=bound))
    print(gather[4](a))
    print(swapped[2](SIMD[DType.int32, 2](7, 9)))
    comptime folded = reversed4()
    print(folded)
