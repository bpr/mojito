# shuffle gathers lanes by compile-time indices: reversal, repetition, a
# narrower or width-1 result, and bool-mask shuffles all work; the mask
# length is itself a SIMD width.
def main():
    var v = SIMD[DType.int32, 4](10, 20, 30, 40)
    print(v.shuffle[3, 2, 1, 0]())
    print(v.shuffle[1, 1]())
    print(v.shuffle[0]())
    var f = SIMD[DType.float32, 2](1.5, 2.5)
    print(f.shuffle[1, 0]())
    var m = v < SIMD[DType.int32, 4](25, 25, 25, 25)
    print(m.shuffle[3, 0]())
