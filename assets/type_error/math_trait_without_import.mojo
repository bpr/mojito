# expect: unknown trait 'Ceilable'
# Only `Absable`, `Powable`, and `Roundable` of the `std.math` operation
# traits are prelude names, as upstream's: `Ceilable` needs
# `from std.math import Ceilable`.
def cl[T: Ceilable](x: T) -> T:
    return x.__ceil__()


def main():
    print(cl(2))
