# Probe: is a generic `def` over an `Iterable` parameter served by its
# template when its signature does not spell the element type?
#
# `count` names `C.Element` only as the type of its loop variable. The
# elaborator solves an associated type by unifying a signature against a
# call's types, so it has none to instantiate the template with, and the call
# at a loan-carrying argument keeps its clone.
#
# The pin (2026-09-21) rejects the program: its `Iterable` proves no
# destructor for the loop's element. Mojito prints `1`, and the clone
# `count$y15:List[Span[Int]]` is the residue.
def count[C: Iterable](items: C) -> Int:
    var n = 0
    for item in items:
        n += 1
    return n


def run(xs: List[Int]):
    var spans = List[Span[Int, origin_of(xs)]]()
    spans.append(Span(xs))
    print(count(spans))


def main():
    run([1, 2, 3])
