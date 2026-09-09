# `DType.int` lanes are 64-bit: multi-lane `+`, `*`, and `<<` wrap exactly
# like the native `Int`, on every backend.
def main():
    var big = SIMD[DType.int, 2](9223372036854775807, 1)
    print(big + SIMD[DType.int, 2](1, 1))
    print(big * big)
    print(SIMD[DType.int, 2](1099511627776, 3) * SIMD[DType.int, 2](1099511627776, 3))
    print(SIMD[DType.int, 2](1, -1) << 63)
    print((big + 1).reduce_add(), (big * 2).reduce_min())
    var w: SIMD[DType.int, 2] = big
    w[0] += 2
    print(w, w[0] == -9223372036854775807)
