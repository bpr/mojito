# `pop.cmp` names its predicate as the `pred` attribute; the pin's verifier
# rejects a compare without one.
# expect: invalid call to 'pop.cmp': op requires attribute 'pred'
def main():
    var a = SIMD[DType.int32, 2](1, 2)
    var m = __mlir_op.`pop.cmp`(a._mlir_value, a._mlir_value)
    print(SIMD[DType.bool, 2](mlir_value=m))
