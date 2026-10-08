# Pin gap probe (Mojo 1.2.0.dev2026092105): a trait's compile-time value
# member read through a bound type parameter at run time, in a generic
# `def` (`T.K`) and in a generic struct's method (`Self.T.K`). The pin
# prints `7`, `10`, and `7`. Mojito fails MIR verification on `T.K` with
# "place projects unknown field 'K' of 'A'", and rejects `Self.T.K` with
# "'Self.T' is not a type parameter of the enclosing struct". Roadmap R484.
trait HasK:
    comptime K: Int


@fieldwise_init
struct A(HasK):
    comptime K = 7


def read[T: HasK]():
    print(T.K)


def offset[n: Int, T: HasK]():
    comptime k = T.K + n
    print(k)


struct S[T: HasK]:
    def __init__(out self):
        pass

    def f(self):
        print(Self.T.K)


def main():
    read[A]()
    offset[3, A]()
    S[A]().f()
