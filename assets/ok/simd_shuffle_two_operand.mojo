# `shuffle[*mask](other)` gathers the receiver's lanes followed by `other`'s,
# each mask index below twice the width: upstream's declared overload over
# `_shuffle_list` and `pop.simd.shuffle`.


def blend[dt: DType, w: SIMDLength](a: SIMD[dt, w], b: SIMD[dt, w]) -> SIMD[dt, w]:
    return a.shuffle[3, 0](b)


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    var w = SIMD[DType.int32, 4](5, 6, 7, 8)
    print(v.shuffle[7, 2, 5, 0](w))
    print(v.shuffle[4, 5, 6, 7](w))
    var f = SIMD[DType.float32, 2](1.5, 2.5)
    var g = SIMD[DType.float32, 2](3.5, 4.5)
    print(f.shuffle[2, 1](g))
    print(blend(SIMD[DType.int64, 2](10, 20), SIMD[DType.int64, 2](30, 40)))
    comptime c = SIMD[DType.int32, 2](1, 2).shuffle[3, 0](
        SIMD[DType.int32, 2](3, 4)
    )
    print(c)
    print(v.join(w).shuffle[0, 7, 1, 6, 2, 5, 3, 4]())
