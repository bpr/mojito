# A compile-time call of a callee with a `ref` parameter passes its argument
# stored in compile-time memory, and a callee's origin parameter binds that
# memory's origin: in a signature, at struct scope, and for a struct result.


@fieldwise_init
struct W(Copyable, ImplicitlyCopyable):
    var v: Int


def bc(ref s: String) -> Int:
    return s.byte_length()


def mk(ref s: String) -> W:
    return W(s.byte_length())


def mk_at[o: Origin](ref[o] s: String) -> W:
    return W(s.byte_length() * 10)


struct H[n: Int]:
    comptime m = bc(String("abc")) + Self.n


def k() -> SIMD[DType.int8, bc(String("ab"))]:
    return SIMD[DType.int8, bc(String("ab"))](1)


def g[w: W]():
    print(w.v)


def f[c: Span[Int, _]]():
    print(len(c), c[1])


def spans[l: List[Int]]():
    f[Span(l)]()


def main():
    print(H[1].m)
    print(k())
    g[mk(String("abcde"))]()
    g[mk_at(String("abc"))]()
    spans[[4, 5, 6]]()
    comptime n = bc(String("abcd"))
    print(n)
