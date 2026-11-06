# A generator type's result is a type expression, so a `&` composition after
# it is the whole result (`__generator_type[T: Copyable] Copyable &
# Defaultable`), not further bounds of the parameter it types: an
# application of the parameter conforms to every composed trait.
def need_default[T: Defaultable]() -> Int:
    return 1


def need_copy[T: Copyable]() -> Int:
    return 2


def both[G: __generator_type[T: Copyable] Copyable & Defaultable]() -> Int:
    return need_default[G[Int]]() + need_copy[G[Int]]()


def forwarded[G: __generator_type[T: Copyable] Copyable & Defaultable]() -> Int:
    return both[G]() * 10


def main():
    print("composed")
