# `IntLiteral` does not list `Equatable`, so a literal type bound to an
# `Equatable` parameter is rejected.
# expect: does not conform to trait 'Equatable'
def f[T: Equatable](a: T) -> Bool:
    return a == a


def main():
    comptime x = 3
    print(f[type_of(x)](x))
