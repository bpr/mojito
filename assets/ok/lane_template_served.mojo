# A uniquely named `def` keyed on a `DType` binder or a lane width is served by
# its template: the body is checked once with the lane slots symbolic, MIR
# carries the slots in its register types and SIMD instructions, and the
# elaborator closes them per call, binding the lane from the brackets or from
# the argument's own slots, where a width is spelled in the brackets, as the
# pin requires. A construction, a cast, a `to_bits` with and
# without its target, a lane count, a dtype read, a lane index, a reduction,
# a literal beside the lane, a scalar splat, a `comptime if` over the binder,
# and a `DType` returned as a value all cross the waist symbolic.
def splat[dt: DType, w: Int](x: Scalar[dt]) -> SIMD[dt, w]:
    var v: SIMD[dt, w] = x
    return v + 1


def lanes[dt: DType, w: Int](v: SIMD[dt, w]) -> Int:
    return v.length + len(v) * 10


def widened[dt: DType](a: Scalar[dt]) -> Scalar[DType.int64]:
    return a.cast[DType.int64]() * 2


def bits[dt: DType, w: Int](v: SIMD[dt, w]) -> UInt64:
    return v.to_bits().cast[DType.uint64]().reduce_add() + v.to_bits[DType.uint64]()[0]


def kind[dt: DType](a: Scalar[dt]) -> Int:
    comptime if dt.is_floating_point():
        return 1
    comptime if dt == DType.int32:
        return 2
    return 3


def lane_of[dt: DType](a: Scalar[dt]) -> DType:
    return a.dtype


def first[dt: DType, w: Int](v: SIMD[dt, w]) -> Scalar[dt]:
    return v[0] + v[w - 1]


def main():
    print(splat[DType.int32, 4](Int32(6)))
    print(splat[DType.float32, 2](Float32(1.5)))
    var v = SIMD[DType.int16, 4](1, 2, 3, 4)
    print(lanes[DType.int16, 4](v), lanes[DType.int8, 1](Int8(7)))
    print(widened(Int8(21)), widened(Float32(2.5)))
    print(bits[DType.uint8, 2](SIMD[DType.uint8, 2](3, 4)), bits[DType.float32, 1](Float32(1.0)))
    print(kind(Float64(1.0)), kind(Int32(1)), kind(UInt8(1)))
    print(lane_of(Int32(3)), lane_of(Float32(1.0)))
    print(first[DType.int16, 4](v), first[DType.float64, 2](SIMD[DType.float64, 2](1.5, 2.5)))
