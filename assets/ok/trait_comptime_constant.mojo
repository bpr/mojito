# A trait, or a `&` composition of traits and trait constants, binds a
# `comptime` constant of the trait metatype at module or local scope. A
# trait-valued parameter takes it, a default names it, `type_of` reads its
# metatype, a bound reads its traits, and so does `conforms_to`.
trait Show:
    def show(self) -> Int: ...


comptime MyTr = Copyable & Movable
comptime T = Copyable
comptime U: type_of(AnyType) = Movable
comptime Q: type_of(Movable) = Copyable
comptime S = Show & Copyable
comptime A = Copyable
comptime B = A & Hashable
comptime D = Movable


@fieldwise_init
struct P(Copyable, Show):
    var a: Int

    def show(self) -> Int:
        return self.a


def g[Tr: type_of(Movable)]() -> Int:
    return 1


def any_trait[Tr: type_of(AnyType)]() -> Int:
    return 2


def shown[Tr: type_of(Show)]() -> Int:
    return 3


def defaulted[Tr: type_of(AnyType) = D]() -> Int:
    return 4


def ident[X: MyTr](x: X) -> X:
    return x.copy()


def hashed[X: B](x: X) -> UInt64:
    return hash(x)


def show_it[X: S](x: X) -> Int:
    return x.show()


def main():
    print(g[MyTr](), g[T](), g[Q](), any_trait[U](), any_trait[B]())
    print(shown[S](), defaulted())
    comptime L = Copyable & Movable
    comptime M: type_of(AnyType) = Copyable
    comptime N = L & Hashable
    comptime V = type_of(L)
    print(g[L](), any_trait[M](), g[N]())
    print(ident(3), hashed(5) == hash(5), show_it(P(7)))
    print(conforms_to(Int, MyTr), conforms_to(Int, L), conforms_to(P, S))
    print(conforms_to(P, B), conforms_to(Int, N))
