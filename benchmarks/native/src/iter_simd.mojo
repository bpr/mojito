# Benchmark: iterator loops over ranges and Lists plus SIMD arithmetic
# with a SIMD[DType.float64, 4] accumulator.
def main():
    var data: List[Float64] = []
    var i: Int = 0
    while i < 400:
        data.append(Float64(i % 31) * 0.5)
        i += 1

    var scalar_sum: Float64 = 0.0
    var r: Int = 0
    while r < 150:
        for x in data:
            scalar_sum += x
        r += 1

    var idx_sum: Int = 0
    for k in range(0, 400000, 3):
        idx_sum += k % 11

    var acc = SIMD[DType.float64, 4](0.0, 0.0, 0.0, 0.0)
    var lane = SIMD[DType.float64, 4](1.0, 2.0, 3.0, 4.0)
    var t: Int = 0
    while t < 2000000:
        acc = acc + lane * 0.5
        t += 1

    print("scalar_sum:", scalar_sum)
    print("idx_sum:", idx_sum)
    print("acc:", acc.reduce_add())
