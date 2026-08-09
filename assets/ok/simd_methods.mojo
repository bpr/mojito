# Elementwise negation, mask select, and lane reductions on SIMD values:
# integer reductions wrap at the element width, DType.int reductions
# canonicalize to the native Int, and bool masks reduce with and/or.
def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(-v)
    print(v.reduce_add())
    print(v.reduce_mul())
    print(v.reduce_min())
    print(v.reduce_max())
    var m = v < SIMD[DType.int32, 4](3, 3, 3, 3)
    print(m.select(v, -v))
    print(m.select(v, 0))
    print(m.reduce_and())
    print(m.reduce_or())
    var f = SIMD[DType.float32, 2](1.5, -2.5)
    print(-f)
    print(f.reduce_add())
    print(SIMD[DType.uint8, 2](200, 100).reduce_add())
    var total: Int = SIMD[DType.int, 4](1, 2, 3, 4).reduce_add()
    print(total + 1)
