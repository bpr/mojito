# A keyed `def` holding a value built by a value-shaped construction
# (`SIMD[DType.int64, w](v)`, `Scalar[dt](v)`) applies operators,
# reductions, casts, lane counts, and scalar conversions to it, and derives
# its instances from the template: none of those records anything the
# folded dimensions decide, and a cast or reinterpretation over an open
# lane is checked again against each instance's lane.
def sum_lanes[w: Int](v: Int) -> Int:
    var lanes = SIMD[DType.int64, w](v)
    var twice = lanes + lanes * 3
    var wide = twice.cast[DType.int32]()
    return Int(twice.reduce_add()) + Int(wide.reduce_max()) + wide.length


def scale[dt: DType](v: Int) -> Int:
    var one = Scalar[dt](v)
    var two = one * one - one
    comptime if dt.is_integral():
        return Int(two) + Int(one.to_bits[DType.uint64]())
    else:
        return Int(two.cast[DType.int64]())


def main():
    print(sum_lanes[4](2), sum_lanes[2](9))
    print(scale[DType.int16](3), scale[DType.float32](4), scale[DType.int](5))
