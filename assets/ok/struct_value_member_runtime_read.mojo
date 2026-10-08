# A type's compile-time value member read at run time, as the pin reads it:
# through a bound type parameter whose instance computes the member from its
# own parameters (`B[4]`), on a struct by name (`A.K`, `B[4].K`), in a bracket
# argument (`V[A.K]`), in a method (`Self.K`), and for Float, String, and
# struct-valued members, by name and by the leading-dot form (`.RED`). A
# trait's `String` requirement is witnessed by a string literal.
trait HasK:
    comptime K: Int


trait Named:
    comptime NAME: String


@fieldwise_init
struct A(HasK, Named):
    comptime K = 7
    comptime NAME = "a"
    comptime RATIO = 1.5


struct B[n: Int](HasK):
    comptime K = Self.n + 1

    def __init__(out self):
        pass


struct Buffer[n: Int]:
    comptime size = Self.n * 2

    def __init__(out self):
        pass

    def size_of(self) -> Int:
        return Self.size


struct V[n: Int]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.n


@fieldwise_init
struct Color(Copyable, ImplicitlyCopyable):
    var v: Int
    comptime RED = Color(1)


def pick() -> Color:
    return .RED


def read[T: HasK]() -> Int:
    return T.K


def name[T: Named]():
    var s = T.NAME
    print(s)


def main():
    print(read[A](), read[B[4]]())
    print(A.K, B[4].K)
    comptime k = A.K
    var x = A.K
    print(k + x)
    print(V[A.K]().get())
    print(Buffer[3]().size_of())
    print(A.RATIO, A.NAME)
    name[A]()
    var c = Color.RED
    print(c.v, pick().v)
