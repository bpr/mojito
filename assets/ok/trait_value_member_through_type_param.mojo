# A trait's compile-time value member read at run time through a bound type
# parameter: in a generic `def` (`T.K`), in a compile-time expression over it
# (`comptime k = T.K + n`), and in a generic struct's method (`Self.T.K`).
# The read is a parameter value the elaborator answers per instance, so the
# output pins `7`, `10`, and `7`, as at the pin.
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
