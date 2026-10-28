# `pop.select` takes a `!kgen.scalar<bool>` condition; the pin's verifier
# rejects a two-lane `bool` mask, which `pop.simd.select` takes.
# expect: invalid call to 'pop.select': op operand #0 must be !kgen.scalar<bool>
def main():
    var m = SIMD[DType.bool, 2](True, False)
    var r = __mlir_op.`pop.select`(m._mlir_value, Int(1), Int(2))
    print(r)
