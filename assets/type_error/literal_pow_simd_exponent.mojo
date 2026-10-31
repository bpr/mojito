# A float literal base takes `Float64`'s `__pow__` before the vector's
# `__rpow__`, and that `__pow__`'s constraint rejects a two-lane exponent.
# expect: constraint failed: The SIMD types must be identical or else the exponent must be a scalar.
def main():
    var v = SIMD[DType.float64, 2](2, 3)
    print(2.0 ** v)
