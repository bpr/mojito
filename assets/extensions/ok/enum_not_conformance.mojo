# An `enum` opts out of its implicit value conformances with `not`: without
# `ImplicitlyCopyable` it copies only with `.copy()`, without `Copyable` it
# only moves, and without `Movable` it stays where it is materialized. A
# case reaches run time through `materialize`, and an opted-out derived
# conformance (`not Hashable`) derives nothing.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum Explicit(not ImplicitlyCopyable, Equatable):
    case one, two


enum Unique(not Copyable):
    case first, second


enum Pinned(not Movable, Writable):
    case here, there


enum Color(not Hashable, Equatable):
    case red, green


def name(pinned: Pinned) -> String:
    return String(pinned)


def take(var unique: Unique) -> Bool:
    if case .second = unique:
        return True
    return False


def main():
    var explicit = materialize[Explicit.two]()
    var copy = explicit.copy()
    match copy:
        case .one:
            print("one")
        case .two:
            print("two")
    print(explicit == copy, copy == materialize[Explicit.one]())

    var unique = materialize[Unique.second]()
    var moved = unique^
    print(take(moved^))

    var pinned = materialize[Pinned.there]()
    print(name(pinned), pinned)

    print(Color.red == Color.green, Color.green == Color.green)
