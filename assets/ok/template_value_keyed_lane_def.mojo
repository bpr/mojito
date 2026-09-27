# A `def` keyed on a `DType` binder, or using a parameter as a lane width,
# with no compile-time control flow is still specialized per call; source
# validation checks its template with the parameters symbolic, so each
# instance derives its facts from it.
def lane[dt: DType](v: Int) -> Int:
    var one = Scalar[dt](v)
    return len(one) + v


def wide[w: Int](v: Int) -> Int:
    var lanes = SIMD[DType.int32, w](v)
    return len(lanes)


def both[dt: DType, w: Int](v: Int) -> Int:
    var lanes = SIMD[dt, w](v)
    return len(lanes) * 10 + v


def main():
    print(lane[DType.int16](2), lane[DType.float32](9))
    print(wide[4](2), wide[2](9))
    print(both[DType.int16, 4](2), both[DType.uint8, 8](1))
