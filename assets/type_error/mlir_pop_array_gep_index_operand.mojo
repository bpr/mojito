# `pop.array.gep`'s element operand is an `index`, so an integer literal
# is the pin's verifier error.
# expect: invalid call to 'pop.array.gep': op operand #1 must be index
def main():
    var a = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<2, `, Int, `>`]
    ](Int(1))
    var p = __mlir_op.`pop.array.gep`(Pointer(to=a)._get_kgen_pointer(), 0)
