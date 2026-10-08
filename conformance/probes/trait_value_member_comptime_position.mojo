# Pin gap probe (Mojo 1.2.0.dev2026092105): a trait's compile-time value
# member read through a bound type parameter in a compile-time position of a
# template: a `comptime if` condition, a `comptime for` range bound in a
# method with a binder of its own, and a lane width. The pin prints `big`,
# `[1, 1]`, `0`, `1`, `2`. Mojito rejects the condition and the width with
# "not a compile-time Int constant: unsupported associated comptime member
# access", and the method's call with "function instantiation of `S.m`
# failed: S.m: not a compile-time value: 'T' is not a compile-time type".
# Roadmap R500.
trait HasK:
    comptime K: Int


struct Q(HasK):
    comptime K = 3


def g[T: HasK]():
    comptime if T.K > 2:
        print("big")
    var x = SIMD[DType.int32, T.K - 1](1)
    print(x)


struct S:
    def __init__(out self):
        pass

    def m[T: HasK](self):
        comptime for i in range(T.K):
            print(i)


def main():
    g[Q]()
    S().m[Q]()
