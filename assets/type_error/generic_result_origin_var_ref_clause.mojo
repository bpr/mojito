# A `ref [...]` result clause names an owned parameter of a generic type,
# which might expand to a register-passable type with no memory.
# expect: cannot return 'items's origin, because it might expand to a RegisterPassable type
def keep[T: Copyable](var items: T) -> ref[origin_of(items)] T:
    return items


def main():
    print(1)
