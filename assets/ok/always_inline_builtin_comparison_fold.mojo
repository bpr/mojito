# `SIMD`'s comparison members are `@always_inline("builtin")` bodies over
# `pop.cmp` and the lane reductions, which a compile-time call folds where it
# is spelled: `v.__lt__(w)` is `v < w`, `v.gt(w)` is `w.lt(v)`, and a
# one-lane `__bool__` is `!= 0`, so each pair below is one type.


struct Tag[b: Bool](Movable):
    def __init__(out self):
        pass


struct Mask[m: SIMD[DType.bool, 4]](Movable):
    def __init__(out self):
        pass


@always_inline("builtin")
def any_lane(m: Scalar[DType.bool]) -> Bool:
    return Bool(mlir_value=__mlir_op.`pop.simd.reduce_or`(m._mlir_value))


@always_inline("builtin")
def every_lane(m: Scalar[DType.bool]) -> Bool:
    return Bool(mlir_value=__mlir_op.`pop.simd.reduce_and`(m._mlir_value))


def scalars[v: Scalar[DType.int32], w: Scalar[DType.int32]]():
    var lt: Tag[v.__lt__(w)] = Tag[v < w]()
    var gt: Tag[v.__gt__(w)] = Tag[w < v]()
    var ge: Tag[v.__ge__(w)] = Tag[w <= v]()
    var eq: Tag[v.__eq__(w)] = Tag[v == w]()
    var ne: Tag[v.__ne__(w)] = Tag[v != w]()
    var truth: Tag[v.__bool__()] = Tag[v != 0]()
    var through: Tag[v.lt(w).__bool__()] = Tag[v < w]()
    var any_eq: Tag[any_lane(v.eq(w))] = Tag[v == w]()
    var any_ne: Tag[any_lane(v.ne(w))] = Tag[v != w]()
    var one_lane: Tag[any_lane(v.lt(w))] = Tag[every_lane(v.lt(w))]()
    _ = any_eq^
    _ = any_ne^
    _ = one_lane^
    _ = lt^
    _ = gt^
    _ = ge^
    _ = eq^
    _ = ne^
    _ = truth^
    _ = through^


def floats[v: Float32, w: Float32]():
    var lt: Tag[v.__lt__(w)] = Tag[v < w]()
    var ne: Tag[v.__ne__(w)] = Tag[v != w]()
    var truth: Tag[v.__bool__()] = Tag[v != 0]()
    _ = lt^
    _ = ne^
    _ = truth^


def vectors[v: SIMD[DType.int32, 4], w: SIMD[DType.int32, 4]]():
    var eq: Tag[v.__eq__(w)] = Tag[v == w]()
    var ne: Tag[v.__ne__(w)] = Tag[v != w]()
    var lt: Mask[v.lt(w)] = Mask[w.gt(v)]()
    var ge: Mask[v.ge(w)] = Mask[w.le(v)]()
    var same: Mask[v.eq(w)] = Mask[w.eq(v)]()
    _ = eq^
    _ = ne^
    _ = lt^
    _ = ge^
    _ = same^


def main():
    scalars[Scalar[DType.int32](1), Scalar[DType.int32](2)]()
    floats[Float32(1.5), Float32(-2)]()
    vectors[SIMD[DType.int32, 4](1, 2, 3, 4), SIMD[DType.int32, 4](4, 3, 2, 1)]()
    comptime v = SIMD[DType.int32, 4](1, 2, 3, 4)
    comptime w = SIMD[DType.int32, 4](4, 3, 2, 1)
    comptime m = v.lt(w)
    comptime e = v.__eq__(v)
    comptime z = Float32(0)
    comptime nan = z / z
    comptime truthy = nan.__bool__()
    comptime unequal = nan.ne(nan)
    print(m, e, w.__ne__(v), truthy, unequal)
