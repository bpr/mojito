# A bracket list binds as Mojo binds it: positional arguments first, each to
# the next parameter that is not infer-only, then each keyword by name, an
# infer-only binder included, wherever the keyword is written.
def f[T: Writable & ImplicitlyCopyable, //, n: Int](x: T) -> Int:
    print(x)
    return n


def h[T: Writable & ImplicitlyCopyable, //, *vs: T]() -> Int:
    return len(vs)


def g[a: Int, b: Int]() -> Int:
    return a * 10 + b


def r[T: Writable & ImplicitlyCopyable, o: ImmOrigin, //, n: Int](
    ref[o] x: T,
) -> Int:
    print(x)
    return n


struct PL[T: Writable & ImplicitlyCopyable, //, *vs: T]:
    def __init__(out self):
        pass

    def size(self) -> Int:
        return len(Self.vs)


struct B[d: DType, //, n: Int]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.n


struct Box[Tr: type_of(AnyType), //, n: Int]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.n


struct C[a: Int, b: Int]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.a * 10 + Self.b


struct S:
    def __init__(out self):
        pass

    def m[T: Writable & ImplicitlyCopyable, //, n: Int](self, x: T) -> Int:
        print(x)
        return n


def main():
    print(f[T=Int, n=3](1))
    print(f[T=Float64, n=3](1))
    print(f[T=Int, 3](1))
    print(PL[T=Int]().size())
    print(PL[T=Int, 1, 2]().size())
    print(h[T=Int, 4, 5]())
    print(B[d=DType.int8, 4]().get())
    var b: B[d=DType.int8, 5] = B[d=DType.int8, 5]()
    print(b.get())
    print(Box[Tr=Copyable, 3]().get())
    print(S().m[T=Int, n=5](1))
    print(C[b=2, 1]().get())
    print(g[b=2, 1]())
    var v = 7
    print(r[T=Int, n=2](v))
    print(r[o=origin_of(v), n=4](v))
