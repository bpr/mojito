# A compile-time-keyed `def` constructing a `SIMD` value whose dtype or width
# names its own value binder (`SIMD[dt, w](v)`, `Scalar[dt](v)`) derives its
# instances from the template source validation checked: the template records
# no dimensions for such a construction, and each instance's are its
# substituted construction type's.
def widest[w: Int](v: Int) -> Int:
    var acc = v
    comptime if w > 2:
        var lanes = SIMD[DType.int32, w](3)
        acc += len(lanes)
    return acc


def both[dt: DType, w: Int](v: Int) -> Int:
    var acc = v
    comptime if dt.is_integral():
        var lanes = SIMD[dt, w](v)
        acc += len(lanes) * 10
    else:
        var one = Scalar[dt](v)
        acc += len(one)
    return acc


def main():
    print(widest[4](2), widest[2](9))
    print(both[DType.int16, 4](2), both[DType.float32, 2](9), both[DType.uint8, 8](1))
