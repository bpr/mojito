# expect: cannot transfer out of immutable reference
# The rule holds over a bare type parameter too: the body is rejected once,
# whatever the argument type.
def ident[T: Movable](x: T) -> T:
    return x^


def main():
    print(ident(3))
