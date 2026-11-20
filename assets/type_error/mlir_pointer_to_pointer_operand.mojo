# `lit.ref.to_pointer` converts a `!lit.ref`; the pin rejects a
# `!kgen.pointer` operand.
# expect: invalid call to 'lit.ref.to_pointer': operand #0 must be a '!lit.ref'
def main():
    var x = 4
    var p = Pointer(to=x)
    var k = __mlir_op.`lit.ref.to_pointer`(p._mlir_value)
    print(Pointer[Int, origin_of(x)](_mlir_value=k)[])
