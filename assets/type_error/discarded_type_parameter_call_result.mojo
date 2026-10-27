# `_ = e` over a call result typed by a type parameter the bound does not
# prove `Deinitable` abandons it. `Movable`'s composition closes over
# `AnyType`, which sorts first.
# expect: '(expression temporary)' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'AnyType'
def mk[T: Movable](var a: T) -> T:
    return a^


def g[T: Movable](var a: T):
    _ = mk(a^)


def main():
    g(1)
