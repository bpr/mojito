# A `!kgen.string` parameter binds a KGEN string, not the string literal
# whose `.value` it would be.
# expect: type mismatch for value parameter 's': expected __mlir_type.`!kgen.string`, found StringLiteral
comptime KGENString = __mlir_type.`!kgen.string`


def bind[s: KGENString]():
    pass


def main():
    bind["a"]()
