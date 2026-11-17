# A function's compile-time reference binds a `def(...)` bound to its
# inflated type, as at the pin: an empty struct whose callee is its
# `__call__`, so `F` is zero-sized and trivially register-passable, for a
# bare function, a bracket specialization, a generic function at a generic
# bound, and an overloaded method's bound parameter.
from std.sys import size_of


def dbl(x: Int) -> Int:
    return x * 2


def addn[n: Int](x: Int) -> Int:
    return x + n


def info[F: def(Int) -> Int](f: F) -> Int:
    print(
        size_of[F](),
        conforms_to(F, ImplicitlyCopyable),
        conforms_to(F, TrivialRegisterPassable),
    )
    return f(2)


def use_generic[F: def[n: Int](Int) -> Int](f: F, x: Int) -> Int:
    print(size_of[F]())
    return f[2](x) + f[5](x)


@fieldwise_init
struct V(Copyable):
    var v: Int

    def red[F: def(Int) -> Int](self, func: F) -> Int:
        return func(self.v)

    def red[func: def(Int) thin -> Int](self) -> Int:
        return func(self.v) + 100


def main():
    print(info(dbl))
    print(info(addn[3]))
    print(use_generic(addn, 1))
    print(V(3).red(dbl), V(3).red[dbl]())
