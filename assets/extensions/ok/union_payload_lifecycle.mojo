# A `union` payload's lifecycle: construction moves the payload into the
# storage once, a copy copies it, reassigning another case destroys it, and
# a payload bound without `var` is borrowed, as the pin traces them.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


struct Tracked(ImplicitlyCopyable, Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id
        print("init", id)

    def __init__(out self, *, copy: Self):
        self.id = copy.id + 10
        print("copy", self.id)

    def __init__(out self, *, deinit move: Self):
        self.id = move.id
        print("move", self.id)

    def __deinit__(deinit self):
        print("destroy", self.id)


union Token(ImplicitlyCopyable):
    case eof()
    case tracked(Tracked)


def describe(token: Token) -> String:
    match token:
        case .eof():
            return "eof"
        case .tracked(t):
            return "tracked:" + String(t.id)


def main():
    var token: Token = .tracked(Tracked(1))
    var copy = token.copy()
    print(describe(copy))
    var reassigned: Token = .tracked(Tracked(2))
    print(describe(reassigned))
    reassigned = .eof()
    print(describe(reassigned))
    print(describe(token))
