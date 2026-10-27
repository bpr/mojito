# `pop` arithmetic takes operands of one type: the pin's verifier rejects
# an `int32` vector added to a `float32` one.
# expect: invalid call to 'pop.add': op requires the same type for all operands and results
def main():
    var a = SIMD[DType.int32, 2](1, 2)
    var b = SIMD[DType.float32, 2](1, 2)
    var m = __mlir_op.`pop.add`(a._mlir_value, b._mlir_value)
    print(SIMD[DType.int32, 2](mlir_value=m))
