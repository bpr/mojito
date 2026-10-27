# `pop.simd.select` takes a `bool` mask; the pin's verifier rejects an
# `int32` vector in its place.
# expect: invalid call to 'pop.simd.select': op operand #0 must be !kgen.simd<N, bool>
def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    var r = __mlir_op.`pop.simd.select`(v._mlir_value, v._mlir_value, v._mlir_value)
    print(SIMD[DType.int32, 4](mlir_value=r))
