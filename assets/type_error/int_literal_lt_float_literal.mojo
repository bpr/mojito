# `IntLiteral.__lt__` takes only an `IntLiteral`, and a comparison has no
# reflected dunder, so the left literal materializes to `Int`, whose
# conversion of the `FloatLiteral` right operand requires a float dtype.
# expect: constraint failed: the SIMD type must be floating point
def main():
    print(2 < 2.5)
