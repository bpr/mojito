# A module-qualified trait names the trait in a type-parameter bound, lone
# or composed, as the name `from std.format import Writable` binds does.
import std.format as fm


def f[T: fm.Writable & Copyable](t: T):
    print(t)


def g[T: Copyable & fm.Writable](t: T):
    print(t)


def h[T: fm.Writable](t: T):
    print(t)


def main():
    f(3)
    g(4.5)
    h("x")
