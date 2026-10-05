# expect: function instantiation of `swap` failed: constraint failed: mismatch in the number of elements
# A template-served shuffle's mask has one index per lane of the instance's
# receiver; the instance whose width the mask misses fails, as at the pin.
def swap[dt: DType, w: Int](v: SIMD[dt, w]) -> SIMD[dt, w]:
    return v.shuffle[1, 0]()


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(swap[DType.int32, 4](v))
