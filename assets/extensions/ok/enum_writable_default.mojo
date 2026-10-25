# An `enum` declaring `Writable` without writing it is written as
# upstream's `_enum_reflection_write_to` writes an `EnumLike`: the type name,
# the case, and the payload's own text or `repr` in parentheses.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum Token(ImplicitlyCopyable, Writable):
    case eof
    case identifier(String)
    case integer(Int)
    case span(Int, Int)


def main():
    print(String(Token.eof), String(Token.integer(5)), String(Token.span(1, 2)))
    print(Token.identifier("x"))
    print(repr(Token.identifier("x")), repr(Token.eof))
