# A string literal's type is the `StringLiteral[value]` its text keys: the
# application's annotation takes the literal, a `!kgen.string` parameter is
# solved from it, and its members are the declared struct's bodies.
def g[v: __mlir_type.`!kgen.string`](s: StringLiteral[v]) -> Int:
    return StringLiteral[v]().byte_length()


def h[s: StringLiteral]() -> Int:
    return s.byte_length()


def main():
    var x: StringLiteral["a".value] = "a"
    print(x)
    var ab: StringLiteral["ab".value] = "a" + "b"
    print(ab)
    var y: type_of("hi") = "hi"
    print(y)
    print(StringLiteral["ab".value]()._is_identical("ab"))
    print(StringLiteral["ab".value]()._is_identical("abc"))
    print(g("hello"), h["abcd"]())
    print("abc".byte_length(), Bool(""), Bool("a"))
    print(not "", not "a")
    if "":
        print("empty")
    print("abc".as_string_slice(), len("abcd".as_bytes()))
    print(len(Span[Byte, ImmStaticOrigin](unsafe_ptr="abc".ptr(), length=3)))
    comptime c = "xyz"
    print(c.byte_length())
