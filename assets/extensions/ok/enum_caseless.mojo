# An `enum` without cases has no values, but its type works like any
# other: a `List` of it copies and prints, and a `match` over it needs only
# a fallback.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum Uninhabited(Copyable, Writable):
    pass


def describe(uninhabited: Uninhabited) -> String:
    match uninhabited:
        case _:
            return "unreachable"


def main():
    var values = List[Uninhabited]()
    var copies = values.copy()
    print(len(copies), values)
    var function = describe
    _ = function
