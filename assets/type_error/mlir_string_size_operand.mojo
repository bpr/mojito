# `pop.string.size` reads a `!kgen.string`, not the string literal whose
# `.value` it is.
# expect: operand #0 must be a '__mlir_type.`!kgen.string`', but got 'StringLiteral["ab"]'
def main():
    var n = __mlir_op.`pop.string.size`("ab")
