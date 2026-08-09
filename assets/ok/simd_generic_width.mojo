# A generic def may take its SIMD width as a SIMDLength value parameter:
# each call monomorphizes, so the width is a concrete compile-time value in
# every instance (and an invalid width rejects during checked elaboration).
def splat[w: SIMDLength](value: Int) -> SIMD[DType.int32, w]:
    return SIMD[DType.int32, w](value)

def total[w: SIMDLength](v: SIMD[DType.int32, w]) -> Int32:
    return v.reduce_add()

def main():
    var four = splat[4](7)
    print(four)
    print(total[4](four))
    var two = splat[2](3)
    print(two)
    print(total[2](two))
