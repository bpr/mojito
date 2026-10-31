# An integer literal base materializes to `Int`, whose `FloatLiteral`
# `__pow__` overload requires a floating point dtype.
# expect: constraint failed: a float literal exponent requires a floating point SIMD type
def main():
    print(2 ** 0.5)
