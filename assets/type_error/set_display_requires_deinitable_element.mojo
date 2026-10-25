# expect: invalid call to 'Set': violated constraint
# A set display is `Set`'s `__set_literal__` constructor, whose `where`
# clause requires a `Deinitable` element, so a generic display over an
# element type whose bounds do not prove it is rejected at the display, as
# the spelled constructor call is.
from std.collections import Set


def uniq[T: Hashable & Equatable & Copyable](a: T, b: T) -> Set[T]:
    return {a.copy(), b.copy()}


def main():
    print(len(uniq(3, 3)))
