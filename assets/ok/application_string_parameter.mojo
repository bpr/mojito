# A compile-time call applies a function whose runtime parameter is a
# `String`, as at the pin: over a `String` parameter, a literal, and a
# constructed `String`, and an `@implicit` constructor taking a `String`
# converts a parameter argument.
def ln(s: String) -> Int:
    return s.byte_length()


def h[s: String]() -> SIMD[DType.int8, ln(s)]:
    return SIMD[DType.int8, ln(s)](1)


def g() -> SIMD[DType.int8, ln(String("ab"))]:
    return SIMD[DType.int8, ln(String("ab"))](2)


struct S(ImplicitlyCopyable):
    var n: Int

    @implicit
    def __init__(out self, value: String, /):
        self.n = value.byte_length()


def f[w: S]():
    print(w.n)


comptime s: String = "abcd"


def main():
    print(h["ab"]())
    print(SIMD[DType.int8, ln("abcd")](3))
    print(g())
    f[s]()
