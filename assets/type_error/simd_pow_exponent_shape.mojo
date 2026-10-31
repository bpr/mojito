# A vector exponent of another dtype fails `SIMD.__pow__`'s constraint.
# expect: constraint failed: The SIMD types must be identical or else the exponent must be a scalar.
def main():
    var v = SIMD[DType.float64, 2](2, 3)
    print(v ** SIMD[DType.int32, 2](2, -1))
