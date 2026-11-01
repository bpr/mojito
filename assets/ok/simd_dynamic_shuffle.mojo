# _dynamic_shuffle gathers lanes by a run-time uint8 mask. A 16-lane uint8
# receiver on an SSE4 or NEON target takes the pshufb/tbl1 path, widening a
# narrower mask with `join` and splitting a wider one; every other receiver
# takes the unrolled fallback.
def main():
    var t = SIMD[DType.uint8, 16](10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25)
    print(t._dynamic_shuffle(SIMD[DType.uint8, 4](3, 2, 1, 0)))
    print(t._dynamic_shuffle(SIMD[DType.uint8, 16](15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0)))
    print(t._dynamic_shuffle(SIMD[DType.uint8, 32](0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 15, 15, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6)))
    var f = SIMD[DType.float32, 4](1.5, 2.5, 3.5, 4.5)
    print(f._dynamic_shuffle(SIMD[DType.uint8, 8](3, 3, 2, 2, 1, 1, 0, 0)))
