# `IntLiteral.__floordiv__` takes only an `IntLiteral` and `FloatLiteral`
# declares no `__rfloordiv__` taking one, so the left literal materializes
# to `Int`, whose conversion of the `FloatLiteral` right operand requires a
# float dtype.
# expect: constraint failed: the SIMD type must be floating point
def main():
    print(7 // 2.5)
