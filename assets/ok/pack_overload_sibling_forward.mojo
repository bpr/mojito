# One overload of a type-pack `def` forwards its collected pack whole to a
# same-named sibling. The forward binds the declaration whose collector the
# spread follows (`tally(*rest)` has no fixed prefix, so the collector-only
# overload), exactly as the pinned Mojo binds it when it checks the template.
# requires: discovery


def tally[*Ts: Writable](*values: *Ts) -> Int:
    return values.__len__()


def tally[*Ts: Writable](first: Int, *rest: *Ts) -> Int:
    return first + tally(*rest)


def main():
    print(tally(10, "a", 2))
    print(tally(100, 1, 2, 3))
    print(tally(7))
