# shuffle gathers lanes by compile-time indices: reversal and repetition at
# the receiver's own width, over integer, float, and bool-mask lanes. A mask
# that changes the width is Mojito-only (upstream spells the narrowing as
# `slice`), and lives in the `simd-shuffle-width-change` conformance case.
def main():
    var v = SIMD[DType.int32, 4](10, 20, 30, 40)
    print(v.shuffle[3, 2, 1, 0]())
    print(v.shuffle[1, 1, 1, 1]())
    var f = SIMD[DType.float32, 2](1.5, 2.5)
    print(f.shuffle[1, 0]())
    var m = v.lt(SIMD[DType.int32, 4](25, 25, 25, 25))
    print(m.shuffle[3, 2, 1, 0]())
