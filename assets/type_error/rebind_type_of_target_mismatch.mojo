# expect: the input type does not match the result type
# A `type_of(x)` target is the type `x` has, so rebinding a `Float64` to an
# `Int` operand's type is the mismatch upstream reports when it instantiates.
def k(x: Int, y: Float64) -> Int:
    return rebind[type_of(x)](y)


def main():
    print(k(1, 2.0))
