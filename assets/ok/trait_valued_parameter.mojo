trait Shape:
    def area(self) -> Int: ...


def g[Tr: type_of(AnyType)]():
    print("g")


def h[Tr: type_of(AnyType) = Copyable]() -> Int:
    return 7


def k[Tr: type_of(AnyType), n: Int = 3]() -> Int:
    return n


def m[Tr: type_of(Movable)]() -> Int:
    return 1


def f[Tr: type_of(AnyType)]() -> Int:
    return 1


def fwd[Tr: type_of(AnyType)]() -> Int:
    return f[Tr]() + 1


struct W[Tr: type_of(AnyType)]:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def get(self) -> Int:
        return self.x

    def again(self) -> W[Self.Tr]:
        return W[Self.Tr](self.x + f[Self.Tr]())


struct Box[Tr: type_of(AnyType) = Copyable, //, n: Int]:
    var x: Int

    def __init__(out self):
        self.x = Self.n


struct Pair[Tr: type_of(AnyType), n: Int]:
    var x: Int

    def __init__(out self):
        self.x = Self.n


struct S[Tr: type_of(AnyType)]:
    comptime k = 5


def main():
    g[Copyable]()
    g[Tr=Movable]()
    g[Copyable & Hashable]()
    g[AnyType]()
    g[Shape]()
    print(h())
    print(h[Movable]())
    print(k[Movable]())
    print(k[Movable, 9]())
    print(m[ImplicitlyCopyable]())
    print(m[Copyable & Hashable]())
    print(fwd[Copyable]())
    var w = W[Copyable](5)
    var v = W[Movable](6)
    print(w.get(), v.get())
    print(W[Copyable & Hashable](1).again().x)
    print(Box[3]().x)
    print(Pair[Movable, 4]().x)
    print(Pair[Tr=Movable, n=5]().x)
    var p: Pair[Copyable, 6] = Pair[Copyable, 6]()
    print(p.x)
    print(S[Copyable].k)
