# expect: SIMD width 3 is not a positive power of two
# A generic width binds per call. `_simd_construction_checks` folds away, as
# at the pin, but the instance's three-lane splat survives to the MIR
# verifier, Mojito's stand-in for the pin's LLVM lowering, which rejects a
# non-power-of-two vector length.
def splat[w: SIMDLength](value: Int) -> SIMD[DType.int32, w]:
    return SIMD[DType.int32, w](value)

def main():
    var bad = splat[3](7)
    print(bad)
