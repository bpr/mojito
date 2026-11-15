# A union case carries one payload: `Tuple[]` for `()`, the written type for
# one, and a `Tuple` of several. A case pattern's subpatterns other than
# exactly one form a tuple pattern over the payload, so `.eof()` matches the
# empty payload and `.span(a, b)` its elements; one subpattern matches the
# whole payload, and a struct payload destructures by its fields. Case
# patterns are refutable patterns `if case` takes too, and an unapplied case
# is its constructor. `NoneType` is a payload type like any other.
# `union` is the declaration upstream's enum proposal gives cases with
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


@fieldwise_init
struct SourceRange(Copyable, Writable):
    var start: Int
    var end: Int


union Token(Copyable, Writable):
    case eof()
    case identifier(String)
    case integer(Int)
    case span(Int, Int)
    case source_range(SourceRange)


def describe(token: Token) -> String:
    match token:
        case .eof():
            return "eof"
        case .identifier(name):
            return "ident " + name
        case .integer(0):
            return "zero"
        case .integer(value):
            return "int " + String(value)
        case .span(start, end):
            return "span " + String(start) + ".." + String(end)
        case .source_range({start=start, end=end}):
            return "range " + String(start) + ".." + String(end)


union Poll[T: Copyable & Deinitable](Copyable):
    case pending()
    case ready(Self.T)


union Slot(Copyable, Writable):
    case nothing(NoneType)
    case full(Int)


def main():
    print(describe(Token.eof()), describe(Token.identifier("x")))
    print(describe(Token.integer(0)), describe(Token.integer(3)))
    print(describe(Token.span(1, 2)), describe(.source_range(SourceRange(4, 5))))
    print(Token.eof(), Token.span(1, 2), Token.source_range(SourceRange(6, 7)))
    var poll = Poll[Int].ready(42)
    if case .ready(value) = poll:
        print("ready", value)
    var pending = Poll[Int].pending
    poll = pending()
    if case .pending() = poll:
        print("pending")
    var slot = Slot.nothing(None)
    match slot:
        case .nothing(nothing):
            print("nothing", nothing, slot)
        case .full(value):
            print("full", value)
    slot = .full(8)
    if case .full(value) = slot:
        print("full", value, slot)
