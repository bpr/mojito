# An `enum` without cases has no values, but its type works like any
# other: a `List` of it copies and prints, and a `match` over it needs only
# a fallback.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


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
