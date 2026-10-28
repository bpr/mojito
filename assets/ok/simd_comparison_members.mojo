# Upstream's `SIMD` comparison members are library bodies: `__eq__` and
# `__ne__` compare whole vectors, the ordered dunders compare `Scalar`s, and
# `eq` … `le` are `pop.cmp` masks, whose `ne` is the ordered predicate, so a
# NaN lane is unequal to nothing and a NaN is falsy.


def runtime_zero() -> Float64:
    var total = 0.0
    for i in range(2):
        total += Float64(i) - Float64(i)
    return total


def same[T: Equatable](left: T, right: T) -> Bool:
    return left == right


def before[T: Comparable](left: T, right: T) -> Bool:
    return left < right


def main():
    var a = Int32(1)
    var b = Int32(2)
    print(a.__lt__(b), a.__le__(b), a.__gt__(b), a.__ge__(b), a.__eq__(b), a.__ne__(b))
    var n = 3
    print(n.__lt__(4), n.__eq__(3), n.__ne__(3), n.lt(5), n.eq(3))
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    var w = SIMD[DType.int32, 4](1, 0, 3, 5)
    print(v.__eq__(w), v.__ne__(w), v == v, v != w)
    print(v.eq(w), v.ne(w), v.lt(w))
    print(v.le(w), v.gt(w), v.ge(w))
    print(v.lt(3), v.eq(Int32(3)), Int32(3) in v)
    print(same(v, v), same(v, w), before(a, b), before(Float64(2), Float64(1)))
    print(3 < n, 3.0 < Float64(2.5))
    var z = runtime_zero()
    var nan = z / z
    print(nan != nan, nan == nan, nan.ne(nan), nan.eq(nan))
    print(Bool(nan), nan.__bool__())
    if nan:
        print("nan is truthy")
    else:
        print("nan is falsy")
    var lanes = SIMD[DType.float32, 2](Float32(z), 1.0) / SIMD[DType.float32, 2](Float32(z), 1.0)
    print(lanes != lanes, lanes.ne(lanes), Bool(lanes))
