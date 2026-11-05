# `pop.dtype.to_ui8` takes a `!kgen.dtype`: a `UInt8`'s `_mlir_value` is its
# `!kgen.scalar<ui8>` storage, which the pin's verifier rejects.
# expect: invalid call to 'pop.dtype.to_ui8': op operand #0 must be 8-bit integer DType type, but got '!kgen.scalar<ui8>'
def main():
    var c = UInt8(200)
    var d = __mlir_op.`pop.dtype.to_ui8`(c._mlir_value)
