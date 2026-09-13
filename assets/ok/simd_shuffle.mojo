# shuffle gathers lanes by compile-time indices, one per receiver lane:
# reversal and repetition over integer, float, and bool-mask lanes. A gather
# that changes the width is `slice[width, offset=]()` (consecutive lanes) or
# `join` (two vectors end to end); a width-changing `shuffle` mask is the
# `simd-shuffle-width-change` rejection.
def main():
    var v = SIMD[DType.int32, 4](10, 20, 30, 40)
    print(v.shuffle[3, 2, 1, 0]())
    print(v.shuffle[1, 1, 1, 1]())
    var f = SIMD[DType.float32, 2](1.5, 2.5)
    print(f.shuffle[1, 0]())
    var m = v.lt(SIMD[DType.int32, 4](25, 25, 25, 25))
    print(m.shuffle[3, 2, 1, 0]())
    print(v.slice[2](), v.slice[2, offset=1](), v.slice[1, offset=3](), v.slice[4]())
    var w = SIMD[DType.int32, 4](50, 60, 70, 80)
    print(v.join(w))
    print(m.slice[2, offset=2](), m.join(m))
    print(f.join(f).shuffle[3, 2, 1, 0]())
    print(SIMD[DType.int32, 1](7).join(SIMD[DType.int32, 1](8)))
