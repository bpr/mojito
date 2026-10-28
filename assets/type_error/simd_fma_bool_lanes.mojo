# `SIMD.fma` asserts a numeric dtype, so a mask has no fused multiply-add.
# expect: the SIMD type must be numeric
def main():
    var m = SIMD[DType.bool, 2](True, False)
    print(m.fma(m, m))
