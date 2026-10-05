# An annotated vector binding at a symbolic width or dtype takes a numeric
# literal initializer, as upstream's `@implicit SIMD.__init__(IntLiteral)`
# and `(FloatLiteral)` splat it across every lane: the checker records the
# materialization at the symbolic vector type, and each instance splats it at
# its own width — a `comptime for` index's included.
def bumped[n: Int]() -> Int:
    var v: SIMD[DType.int32, n] = 7
    v += 1
    return Int(v.reduce_add())


def mixed[dt: DType, n: Int]() -> Float64:
    var v: SIMD[dt, n] = 2
    var w: SIMD[DType.float64, n] = 1.5
    return Float64(v.reduce_add()) + w.reduce_add()


def take[n: Int](v: SIMD[DType.int32, n]) -> Int:
    return Int(v.reduce_add())


def passed[n: Int]() -> Int:
    return take[n](3)


def indexed[T: AnyType]() -> Int:
    var total = 0
    comptime for i in range(1, 3):
        var v: SIMD[DType.int32, i] = 8
        total += Int(v.reduce_add())
    return total


def main():
    print(bumped[1]())
    print(bumped[2]())
    print(mixed[DType.float32, 2]())
    print(passed[4]())
    print(indexed[Int]())
