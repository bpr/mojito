# Upstream's `std/math` SIMD `max` and `min` bodies (`pop.max`, `pop.min`),
# `pop.cast` to and from `bool` lanes, and `index.cmp`'s unsigned
# predicates, as `_plugin/selector.mojo` spells them.


def simd_max[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width], y: SIMD[dtype, width], /) -> SIMD[dtype, width]:
    return SIMD[dtype, width](
        mlir_value=__mlir_op.`pop.max`(x._mlir_value, y._mlir_value)
    )


def simd_min[
    dtype: DType, width: SIMDLength, //
](x: SIMD[dtype, width], y: SIMD[dtype, width], /) -> SIMD[dtype, width]:
    return SIMD[dtype, width](
        mlir_value=__mlir_op.`pop.min`(x._mlir_value, y._mlir_value)
    )


def to_bool[
    dtype: DType, width: SIMDLength
](x: SIMD[dtype, width]) -> SIMD[DType.bool, width]:
    return SIMD[DType.bool, width](
        mlir_value=__mlir_op.`pop.cast`[
            _type=SIMD[DType.bool, width]._mlir_type
        ](x._mlir_value)
    )


def from_bool[
    target: DType, width: SIMDLength
](x: SIMD[DType.bool, width]) -> SIMD[target, width]:
    return SIMD[target, width](
        mlir_value=__mlir_op.`pop.cast`[_type=SIMD[target, width]._mlir_type](
            x._mlir_value
        )
    )


def unsigned_less(a: Int, b: Int) -> Bool:
    return Bool(
        value=__mlir_op.`index.cmp`[
            pred=__mlir_attr.`#index.cmp_predicate<ult>`
        ](a.__mlir_index__(), b.__mlir_index__())
    )


def unsigned_compares(a: Int, b: Int):
    var x = a.__mlir_index__()
    var y = b.__mlir_index__()
    print(
        unsigned_less(a, b),
        Bool(
            value=__mlir_op.`index.cmp`[
                pred=__mlir_attr.`#index.cmp_predicate<ule>`
            ](x, y)
        ),
        Bool(
            value=__mlir_op.`index.cmp`[
                pred=__mlir_attr.`#index.cmp_predicate<ugt>`
            ](x, y)
        ),
        Bool(
            value=__mlir_op.`index.cmp`[
                pred=__mlir_attr.`#index.cmp_predicate<uge>`
            ](x, y)
        ),
    )


def main():
    var zero = Float32(0.0)
    var a = SIMD[DType.float32, 4](1.0, -2.0, zero / zero, 0.5)
    var b = SIMD[DType.float32, 4](3.0, -5.0, 1.0, 0.25)
    print(simd_max(a, b), simd_min(a, b))
    print(simd_max(Float64(2.5), Float64(-1.0)), simd_min(Int(-3), Int(4)))
    var u = SIMD[DType.uint8, 2](200, 3)
    var v = SIMD[DType.uint8, 2](100, 7)
    print(simd_max(u, v), simd_min(u, v))
    var i = SIMD[DType.int8, 2](-100, 3)
    var j = SIMD[DType.int8, 2](100, 7)
    print(simd_max(i, j), simd_min(i, j))
    var p = SIMD[DType.bool, 4](True, False, True, False)
    var q = SIMD[DType.bool, 4](True, True, False, False)
    print(simd_max(p, q), simd_min(p, q))
    print(to_bool(SIMD[DType.int32, 4](0, 4, 5, -1)))
    print(to_bool(SIMD[DType.uint8, 4](0, 1, 2, 255)))
    print(to_bool(SIMD[DType.float32, 4](0.0, 1.0, 0.5, 1.0)))
    print(to_bool(Int(3)), to_bool(p))
    print(from_bool[DType.int32, 4](p), from_bool[DType.int8, 4](q))
    print(from_bool[DType.float64, 4](p), from_bool[DType.int, 4](q))
    unsigned_compares(-1, 3)
    unsigned_compares(3, -1)
    unsigned_compares(7, 7)
