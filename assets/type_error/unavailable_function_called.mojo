# expect: use g instead
# A call selecting an `@unavailable` function is rejected with the
# decorator's reason.
@unavailable("use g instead")
def f(x: Int) -> Int:
    ...


def g(x: Int) -> Int:
    return x + 1


def main():
    print(f(1))
