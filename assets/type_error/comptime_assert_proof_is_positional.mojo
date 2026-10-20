# expect: does not conform to trait 'Writable'
# A `comptime assert` licenses only the statements after it: a use before it
# is unproven, as at the pin.

def f[T: AnyType](x: T):
    print(x)
    comptime assert conforms_to(T, Writable)


def main():
    f(1)
