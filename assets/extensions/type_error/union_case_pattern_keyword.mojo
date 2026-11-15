# expect: union case patterns do not support keyword arguments
# A union payload is positional; a payload destructured by name is a struct
# matched by its fields (`.at({start=x, end=y})`).
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


union Token(Copyable):
    case span(Int, Int)


def main():
    var token = Token.span(1, 2)
    match token:
        case .span(start=1, end=2):
            pass
        case _:
            pass
