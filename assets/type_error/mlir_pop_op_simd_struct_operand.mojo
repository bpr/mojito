# Probe from R1397: a `pop` op takes a vector's `!kgen.simd` storage, not
# the `SIMD` value holding it, and the pin's verifier rejects the value.
# expect: invalid call to 'pop.add': op operand #0 must be SIMD type, but got 'SIMD[DType.int32, 2]'
def main():
    var a = SIMD[DType.int32, 2](1, 2)
    var b = __mlir_op.`pop.add`(a, a)
    print(b)
