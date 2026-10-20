# expect: does not conform to trait 'Writable'
# A `comptime assert` licenses only the rest of its own block: a use after the
# enclosing `if` is unproven, as at the pin.

def f[T: AnyType](x: T, b: Bool):
    if b:
        comptime assert conforms_to(T, Writable)
    print(x)


def main():
    f(1, True)
