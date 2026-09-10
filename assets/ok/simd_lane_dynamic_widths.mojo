# Run-time lane indexes across the storage widths a scalar entry cannot
# return: float32 lanes written from a loop counter, uint8 lanes written
# from a mirrored index, and int16 lanes rewritten by a descending while.
def main():
    var f = SIMD[DType.float32, 8](0.0)
    for i in range(8):
        f[i] = Float32(i) / 4.0
    var m = SIMD[DType.uint8, 16](0)
    var j = 0
    while j < 16:
        m[j] = m[15 - j] + UInt8(j * 17)
        j += 1
    print(f, m, f[7], m[15])
    var w = SIMD[DType.int16, 4](1, 2, 3, 4)
    var idx = 3
    while idx >= 0:
        w[idx] = w[idx] * 1000
        idx -= 1
    print(w)
