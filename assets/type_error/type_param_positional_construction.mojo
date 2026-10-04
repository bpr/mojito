# expect: a type parameter is constructed only through its bound's initializers
# A type parameter constructs only through its bound's initializers; `Copyable`
# declares none taking a positional argument.
def dup[T: Copyable](x: T) -> T:
    return T(x)


def main():
    print(dup(1))
