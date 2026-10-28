# Upstream's `SIMD` comparison dunders: `==` and `!=` compare whole vectors
# to a `Bool`, `<`, `<=`, `>`, and `>=` give a `Scalar`'s `Bool`, and a
# compile-time comparison folds with its width check dropped, an ordered
# one true where some lane holds.

comptime a = SIMD[DType.int32, 4](1, 2, 3, 4)
comptime any_below = a < SIMD[DType.int32, 4](2)
comptime all_equal = a == SIMD[DType.int32, 4](1, 2, 3, 4)


def same[T: Equatable](left: T, right: T) -> Bool:
    return left == right


def before[T: Comparable](left: T, right: T) -> Bool:
    return left < right


def ordered[w: Int](left: SIMD[DType.int32, w], right: SIMD[DType.int32, w]) -> Bool:
    return left <= right


def below[v: SIMD[DType.int32, 4]]():
    comptime if v < 3:
        print("some lane below 3")
    else:
        print("no lane below 3")


def equal[v: SIMD[DType.int32, 4], w: SIMD[DType.int32, 4]]():
    comptime if v == w:
        print("equal")
    else:
        print("different")


def folded[v: SIMD[DType.int32, 4]]():
    comptime r = v > SIMD[DType.int32, 4](3)
    print(r)


def mask[dt: DType, width: Int](v: SIMD[dt, width]) -> SIMD[DType.bool, width]:
    comptime if width == 1:
        return v == v
    return v.lt(v + 1)


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    var w = SIMD[DType.int32, 4](1, 2, 3, 5)
    print(v == w, v != w, v == v, v != v, v == 1, v != 1)
    var x = Int32(2)
    print(x < Int32(3) or False, x >= Int32(3) and True)
    print(Int32(1) < x < Int32(3), Int32(1) < x <= Int32(1))
    print(same(v, v), same(v, w), same(x, x), before(x, Int32(5)))
    print(ordered[1](Int32(4), Int32(4)), ordered[1](Int32(5), Int32(4)))
    var nan = SIMD[DType.float32, 2](0.0) / SIMD[DType.float32, 2](0.0)
    print(nan == nan, nan != nan)
    var t = Scalar[DType.bool](True)
    var f = Scalar[DType.bool](False)
    print(f < t, t <= f, t > f)
    var m: SIMD[DType.bool, 1] = x < Int32(3)
    print(m)
    if v != w:
        print("vectors differ")
    print(any_below, all_equal)
    below[SIMD[DType.int32, 4](4, 1, 4, 4)]()
    below[SIMD[DType.int32, 4](4, 4, 4, 4)]()
    equal[SIMD[DType.int32, 4](1, 2, 3, 4), SIMD[DType.int32, 4](1, 2, 3, 4)]()
    equal[SIMD[DType.int32, 4](1, 2, 3, 4), SIMD[DType.int32, 4](1, 2, 3, 5)]()
    folded[SIMD[DType.int32, 4](1, 2, 3, 4)]()
    folded[SIMD[DType.int32, 4](1, 2, 3, 3)]()
    print(mask[DType.int32, 1](Int32(7)), mask[DType.int32, 4](v))
