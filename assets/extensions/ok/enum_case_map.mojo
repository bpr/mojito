# An enum case is a `thin` function value: `map` applies it to each element,
# as the pin's enum integration test does (spelled `__enum` there).
enum Token(Copyable):
    case integer(Int)
    case eof


def describe(token: Token) -> String:
    if case .integer(n) = token:
        return "int:" + String(n)
    return "eof"


def main() raises:
    var tokens = List[Token]()
    for token in map[Token.integer]([1, 2, 3]):
        tokens.append(token.copy())
    print(len(tokens), describe(tokens[2]))
