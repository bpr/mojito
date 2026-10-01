# A binding of callable type calls whatever it currently holds. Assigning a
# function to a `mut` parameter writes it back to the caller's binding, also
# when the assigned value is a nested `def` over the enclosing function's
# value parameter, and a local reassigned in place or on one branch calls the
# function it was last given.
def one() -> Int:
    return 1


def two() -> Int:
    return 2


def keep(mut f: def() thin -> Int):
    f = two


def keep_value[n: Int](mut f: def() thin -> Int):
    def inner() -> Int:
        return n

    f = inner


def apply(f: def() thin -> Int) -> Int:
    return f()


def pick(c: Bool) -> Int:
    var f = one
    if c:
        f = two
    return f()


def main():
    var f = one
    print(f())
    keep(f)
    print(f())
    keep_value[7](f)
    print(f())
    print(apply(f))
    keep_value[9](f)
    print(f())

    var g = one
    print(g())
    g = two
    print(g())
    print(pick(True))
    print(pick(False))
