# expect: type mismatch for argument 'x' to 'f'
# A `FloatLiteral` parameter takes only a literal: a `Float64` value has no
# literal value for the parameter's type to carry.
def f(x: FloatLiteral) -> Float64:
    return Float64(x)

def main():
    var n = 0.5
    print(f(n))
