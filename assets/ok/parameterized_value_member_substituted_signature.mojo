# A parameterized value requirement read in a signature (`Box[T.get[0]]`
# through a bound, `Box[Self.get[0]]` in the trait's own method) closes at
# the substitution: the conformer's member bound to the read's arguments,
# under the struct's own arguments.


struct Box[n: Int]:
    def __init__(out self):
        pass


trait Tr:
    comptime get[idx: Int]: Int

    def take(self, b: Box[Self.get[0]]) -> Int:
        ...


struct S(Tr):
    comptime get[idx: Int]: Int = idx + 1

    def __init__(out self):
        pass

    def take(self, b: Box[1]) -> Int:
        return b.n * 10


struct S2[k: Int](Tr):
    comptime get[idx: Int]: Int = idx + Self.k

    def __init__(out self):
        pass

    def take(self, b: Box[Self.k]) -> Int:
        return b.n * 100


def f[T: Tr](x: Box[T.get[0]]):
    print(x.n)


def g[T: Tr](x: Box[T.get[2]]) -> Int:
    return x.n


def h[T: Tr](x: Box[T.get[1] * 2]) -> Int:
    return x.n


def use[T: Tr](t: T, b: Box[T.get[0]]) -> Int:
    return t.take(b)


def main():
    f[S](Box[1]())
    print(g[S2[5]](Box[7]()))
    print(h[S2[3]](Box[8]()))
    print(use(S(), Box[1]()))
    print(use(S2[4](), Box[4]()))
