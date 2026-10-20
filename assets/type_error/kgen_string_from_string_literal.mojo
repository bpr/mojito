# A string literal does not convert to its `!kgen.string` parameter type:
# the KGEN string is read with `.value`.
# expect: type mismatch for comptime 'a': expected __mlir_type.`!kgen.string`, found StringLiteral
comptime KGENString = __mlir_type.`!kgen.string`


def main():
    comptime a: KGENString = "a"
