# A trait's `comptime` requirement may declare a default, which a conformer
# that omits the member takes, read with `Self` as that conformer: through a
# bound (`T.K`), by name (`A.K`), in a compile-time expression, and in a
# trait default method (`Self.K2`). A default may read another member
# (`Self.K * 2`), the conformer's own or defaulted, a refining trait's
# conformer takes its parent's default, and a type-valued requirement's
# default is a type (`Q.E`).
comptime N = 5


trait HasK:
    comptime K: Int = 3
    comptime K2: Int = Self.K * 2
    comptime M: Int = N + 1
    comptime S: String = "dflt"
    comptime B: Bool = True

    def doubled(self) -> Int:
        return Self.K2


@fieldwise_init
struct A(HasK):
    pass


@fieldwise_init
struct B(HasK):
    comptime K = 7
    comptime S = "own"


@fieldwise_init
struct G[n: Int](HasK):
    comptime K = Self.n


trait Derived(HasK):
    pass


@fieldwise_init
struct C(Derived):
    pass


trait HasE:
    comptime E: AnyType = Int


@fieldwise_init
struct Q(HasE):
    pass


def show[T: HasK](t: T):
    print(T.K, T.K2, T.M, T.S, T.B, t.doubled())


def main():
    show(A())
    show(B())
    show(G[5]())
    show(C())
    print(A.K, B.K2, G[4].K2, C.M)
    comptime k = A.K + 1
    print(k)
    var x: Q.E = 3
    print(x)
