# A string literal converts to `StringSpan` through the view's `@implicit`
# `StringLiteral` constructor (upstream's `StaticString` initializer): at a
# view parameter, a view-annotated binding, and by direct construction. The
# literal's bytes live for the whole program, so the view carries no loan.
def byte_count(v: StringSpan) -> Int:
    return v.byte_length()

def main():
    var v: StringSpan = "abc"
    print(v.byte_length(), byte_count("hello"))
    var e = StringSpan("xyz")
    print(e, e.byte_length())
    var s = String("abc")
    print(v == "abc", v == s, v == StringSpan(s), v != e, s == v)
    print("b" in v, "q" in v)
    print(v, e[byte=1:3])
