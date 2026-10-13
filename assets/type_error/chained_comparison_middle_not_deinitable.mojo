# expect: '(expression temporary)' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'Comparable'
# A chained comparison's first middle becomes an owned temporary after the
# first link, which the body must destroy; a `T` not proven `Deinitable`
# cannot be implicitly.
def ordered[T: Comparable & ImplicitlyCopyable](x: T, y: T, z: T) -> Bool:
    return x < y < z


def main():
    print(ordered(1, 2, 3))
