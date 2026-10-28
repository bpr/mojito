# An `index` op takes the builtin `index`: an `Int`'s `_mlir_value` is its
# `!kgen.scalar<index>` storage, which the pin's verifier rejects.
# expect: invalid call to 'index.add': op operand #0 must be index, but got '!kgen.scalar<index>'
def main():
    var a = 7
    print(Int(mlir_value=__mlir_op.`index.add`(a._mlir_value, a._mlir_value)))
