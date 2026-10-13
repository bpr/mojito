# A result may name only the origin of a value with memory to borrow. A read
# parameter of a generic type might be register-passed, so its origin is not
# one a result can name; upstream's `iter()` declares the parameter `ref`.
# expect: cannot return 'items's origin, because it might expand to a RegisterPassable type
def it_of[C: Iterable & Copyable](items: C) -> C.IteratorType[origin_of(items)]:
    return items.__iter__()


def main():
    var xs: List[Int] = [1]
    var it = it_of(xs)
    print(len(xs))
