# `IntLiteral`'s `value` parameter is the `!pop.int_literal` attribute a
# literal's `.value` reads, not the literal itself.
# expect: type mismatch for value parameter 'value': expected __mlir_type.`!pop.int_literal`
def main():
    print(Int(IntLiteral[4]()))
