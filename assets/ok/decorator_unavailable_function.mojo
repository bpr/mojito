# `@unavailable` marks a declaration whose every use is an error: the
# declaration itself is accepted, with the `...` body the pin requires, and
# has no body to lower. Overload selection still sees it, so `f(1.5)` picks
# the available `Float64` overload beside an unavailable `Int` one.
@unavailable("use g instead")
def f(x: Int) -> Int:
    ...


def f(x: Float64) -> Int:
    return 2


@unavailable(use=g)
def h(x: Int) -> Int:
    ...


def g(x: Int) -> Int:
    return x + 1


def main():
    print(g(1))
    print(f(1.5))
