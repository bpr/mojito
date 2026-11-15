# A `union` declaring `Writable` without writing it is written as
# upstream's `_enum_reflection_write_to` writes a `UnionLike`: the type name,
# the case, and the payload's own text or `repr` in parentheses, a tuple
# payload element by element and an empty one as `()`.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union Token(ImplicitlyCopyable, Writable):
    case eof()
    case identifier(String)
    case integer(Int)
    case span(Int, Int)


def main():
    print(String(Token.eof()), String(Token.integer(5)), String(Token.span(1, 2)))
    print(Token.identifier("x"))
    print(repr(Token.identifier("x")), repr(Token.eof()), repr(Token.span(3, 4)))
