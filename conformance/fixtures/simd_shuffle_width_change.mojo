# A `shuffle` mask has one index per receiver lane, so it cannot narrow or
# widen the vector; both compilers reject a two-index mask on four lanes.
# Narrowing is `slice[width, offset=]()` and widening is `join`
# (`assets/ok/simd_shuffle.mojo`).
def main():
    var v = SIMD[DType.int32, 4](10, 20, 30, 40)
    print(v.shuffle[1, 1]())
