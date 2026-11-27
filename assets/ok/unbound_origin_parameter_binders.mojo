# A parameter typed by a struct with an unbound origin (`Span[Byte, _]`, a
# bare `S`) is generic over that origin and its `mut=`, as upstream's
# `addImplicitTypeParams` declares the pair: each call keys its own
# instance, so the body reads the argument's mutability, through a
# conversion too, and a trait requirement spelled so matches its witness.
struct S[mut: Bool, //, T: AnyType, origin: Origin[mut=mut], *, k: Int = 1](
    ImplicitlyCopyable
):
    var a: Int

    def __init__(out self):
        self.a = 0


def freeze(other: S) -> S[other.T, ImmOrigin(other.origin), k=other.k]:
    return S[other.T, ImmOrigin(other.origin), k=other.k]()


def mutability(other: S) -> Bool:
    return other.mut


def spans(s: Span[Byte, _]):
    print(s.mut, s.origin.mut, len(s))


struct V[mut: Bool, //, T: Copyable, origin: Origin[mut=mut]](ImplicitlyCopyable):
    var n: Int

    @implicit
    def __init__(out self, ref[Self.origin] xs: List[Self.T]):
        self.n = len(xs)


def viewed(v: V[Int, _]) -> Bool:
    return v.mut


trait Counted:
    def count(self, v: V[Int, _]) -> Int:
        ...


struct C(Counted):
    var k: Int

    def __init__(out self):
        self.k = 1

    def count(self, v: V[Int, _]) -> Int:
        return v.n + self.k


def counted[X: Counted](x: X, v: V[Int, _]) -> Int:
    return x.count(v)


def main():
    var x = 5
    var s = S[Int, origin_of(x)]()
    print(freeze(s).mut, mutability(s), mutability(freeze(s)))
    var bs: List[Byte] = [1, 2]
    spans(bs)
    spans(Span(bs))
    spans(Span[Byte, ImmOrigin(origin_of(bs))](bs))
    var xs: List[Int] = [1, 2, 3]
    print(viewed(V[Int, origin_of(xs)](xs)), viewed(V[Int, ImmOrigin(origin_of(xs))](xs)))
    print(viewed(xs))
    var c = C()
    print(c.count(V[Int, origin_of(xs)](xs)), counted(c, V[Int, origin_of(xs)](xs)))
