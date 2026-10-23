# expect: 'f' is unavailable, use 'g' instead
# An `@unavailable(use=g)` names its replacement in the rejection.
def g(x: Int) -> Int:
    return x


@unavailable(use=g)
def f(x: Int) -> Int:
    ...


def main():
    print(f(1))
