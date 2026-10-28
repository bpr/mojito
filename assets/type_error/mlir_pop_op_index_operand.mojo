# A `pop` op takes `!kgen.simd` storage: the builtin `index`
# `__mlir_index__` returns is not one, and the pin's verifier rejects it.
# expect: invalid call to 'pop.add': op operand #0 must be SIMD type, but got 'index'
def main():
    var x = Int(7).__mlir_index__()
    print(Int(mlir_value=__mlir_op.`pop.add`(x, x)))
