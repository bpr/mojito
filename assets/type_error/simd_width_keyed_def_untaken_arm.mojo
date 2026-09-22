# expect: type mismatch for variable 's': expected String, found SIMD[dt, width]
# A def whose vector width names its own parameter is validated once with
# `SIMD[dt, width]` symbolic, so the arm `width == 4` never selects is still
# reported.
def total[dt: DType, width: Int](v: SIMD[dt, width]) -> Int:
    comptime if width == 4:
        return 4
    else:
        var s: String = v
        return 0


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(total[DType.int32, 4](v))
