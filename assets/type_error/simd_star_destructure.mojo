# A star target over a vector reads its suffix through a slice getter, which
# `SIMD` does not declare: the pin rejects the slice literal as `idx`.
# expect: cannot be indexed here
def main() raises:
    var [a, *r] = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(a)
