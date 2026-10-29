# An overloaded initializer whose parameter spells a scalar alias and the
# struct's own origin binder (`Span[Byte, Self.origin]`) declares the
# symbol its call selects.
struct V[mut: Bool, //, origin: Origin[mut=mut]](ImplicitlyCopyable):
    var n: Int

    def __init__(out self, *, bytes: Span[Byte, Self.origin]):
        self.n = len(bytes)

    def __init__(out self, k: Int):
        self.n = k


def main():
    var l: List[Byte] = [1, 2, 3]
    var v = V(bytes=Span(l))
    print(v.n, V[origin_of(l)](5).n)
