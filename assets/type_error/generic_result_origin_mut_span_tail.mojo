# A struct origin tail in the result names a `mut` parameter of a generic
# type, which might expand to a register-passable type with no memory.
# expect: cannot return 'items's origin, because it might expand to a RegisterPassable type
def view[T: Copyable](mut items: T) -> Span[T, origin_of(items)]:
    return Span[T, origin_of(items)]()


def main():
    print(1)
