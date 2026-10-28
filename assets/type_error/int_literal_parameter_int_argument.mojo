# expect: type mismatch for argument 'x' to 'f'
# An `IntLiteral` parameter takes only a literal: an `Int` value has no
# literal value for the parameter's type to carry.
def f(x: IntLiteral) -> Int:
    return Int(x)

def main():
    var n = 3
    print(f(n))
