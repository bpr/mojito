# The pin's enums (`Mojo/test/mojo-integration/enum.mojo`) as a union:
# construction by qualified and contextual name, two cases sharing a payload
# type, a several-type payload matched element by element and as one tuple,
# the discriminant, and `ref` and `var` payload bindings.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union Token:
    case eof()
    case identifier(String)
    case integer(Int)
    case span(Int, Int)
    case keyword(String)


def describe(token: Token) -> String:
    match token:
        case .eof():
            return "eof"
        case .identifier(name):
            return "word:" + name
        case .keyword(name):
            return "word:" + name
        case .integer(value):
            return "int:" + String(value)
        case .span(start, end):
            return "span:" + String(start) + ".." + String(end)


def take(token: Token) -> String:
    return describe(token)


def main():
    print(describe(Token.integer(42)), Token.integer(42)._get_sum_discriminant())
    var ident: Token = .identifier("main")
    print(describe(ident), ident._get_sum_discriminant())
    print(describe(Token.span(3, 7)), Token.span(3, 7)._get_sum_discriminant())
    var kw: Token = .keyword("def")
    print(describe(kw), kw._get_sum_discriminant())
    print(take(.integer(1)), take(.span(1, 2)), take(.eof()))
    match Token.span(4, 6):
        case .span(pair):
            print("pair", pair[0], pair[1])
        case _:
            pass
    match Token.span(5, 9):
        case .span((start, end)):
            print("nested", start, end)
        case _:
            pass

    var token: Token = .identifier("count")
    match token:
        case .identifier(ref name):
            name += "er"
        case _:
            pass
    print(describe(token))
    var copied: Token = .integer(1)
    match copied:
        case .integer(var value):
            value += 1
            print("local", value)
        case _:
            pass
    print(describe(copied))
