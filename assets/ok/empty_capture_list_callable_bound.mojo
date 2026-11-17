# A lambda or nested `def` with an empty capture list (`{}`) passed at a
# `def(...)` callable bound is capture-free, as at the pin: it binds as its
# generator's symbol, so the bound's type is zero-sized, the same as with
# the capture list omitted.
from std.sys import size_of


def transform[F: def(x: Int) -> Int](f: F, v: Int) -> Int:
    print(size_of[F]())
    return f(v)


def main():
    def triple(x: Int) {} -> Int:
        return x * 3

    print(transform(lambda (x: Int) {} -> Int: x * 3, 5))
    print(transform(triple, 7))
    print(transform(lambda (x: Int) -> Int: x * 3, 9))
