# expect: positive power of two
# A generic width binds per call during checked elaboration, where the
# ordinary power-of-two validation rejects it — not at a late VM operation.
def splat[w: SIMDLength](value: Int) -> SIMD[DType.int32, w]:
    return SIMD[DType.int32, w](value)

def main():
    var bad = splat[3](7)
    print(bad)
