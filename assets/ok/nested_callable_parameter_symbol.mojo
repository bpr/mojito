# A capture-free nested `def` or lambda passed to a callable parameter is
# the symbol of its generator applied to the binders in scope where it is
# declared, as the pin keys `g[inner]` by the nested generator applied to
# the enclosing instance: `S[inner]` is one instance per `outer[n]`, two
# nested `def`s are two instances in one body, and at a `def(...)` bound
# each inflates to a zero-sized function type.
from std.sys import size_of


def g[f: def() thin -> Int]() -> Int:
    return f()


struct S[f: def() thin -> Int]:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def get(self) -> Int:
        return Self.f() + self.x


def info[F: def(Int) -> Int](f: F):
    print(size_of[F](), f(1))


def outer[n: Int]():
    def inner() -> Int:
        return n

    def hundred() -> Int:
        return n * 100

    print(S[inner](1).get(), S[hundred](2).get())
    print(g[inner](), g[lambda -> Int: n + 10]())

    def step(x: Int) -> Int:
        return x + n

    info(step)
    info(lambda (x: Int) -> Int: x * n)


def main():
    outer[4]()
    outer[5]()

    def seven() -> Int:
        return 7

    print(S[seven](1).get(), S[lambda -> Int: 5](1).get())
    print(g[seven](), g[lambda -> Int: 3]())
