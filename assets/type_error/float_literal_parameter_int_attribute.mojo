# `FloatLiteral`'s `value` parameter is a `!pop.float_literal`; an integer
# literal's `!pop.int_literal` attribute is another type.
# expect: type mismatch for value parameter 'value': expected __mlir_type.`!pop.float_literal`, found __mlir_type.`!pop.int_literal`
def main():
    print(Int(FloatLiteral[(2).value]()))
