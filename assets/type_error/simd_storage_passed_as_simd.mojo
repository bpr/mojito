# Probe from R1397: a vector's `_mlir_value` is its `!kgen.simd` storage, a
# type of its own, so it does not pass where the `SIMD` it stores is taken.
# expect: type mismatch for argument 'x' to 'show': expected SIMD[DType.int32, 2], found __mlir_type.`!kgen.simd<2, si32>`
def show(x: SIMD[DType.int32, 2]):
    print(x)


def main():
    var v = SIMD[DType.int32, 2](1, 2)
    show(v._mlir_value)
