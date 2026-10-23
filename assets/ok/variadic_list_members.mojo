# A homogeneous collector is upstream's `VariadicList` in its callee's body:
# its own `__len__` answers, an indexed element of a `var` collector is a
# place written through `__getitem__`'s reference, and a `ref` loop mutates
# each element in place. Any element a mutable-reference `__getitem__`
# reaches with no `__setitem__` beside it is such a place (`Span`).
@fieldwise_init
struct M(Movable):
    var v: Int


@fieldwise_init
struct C(Copyable):
    var v: Int


def count(*xs: Int) -> Int:
    return xs.__len__()


def refs(var *xs: M):
    xs[0].v += 7
    for ref x in xs:
        x.v += 10
    var a = xs[0].v
    var b = xs[1].v
    print(a, b)


def span_place():
    var l: List[C] = [C(1), C(2)]
    var s = Span(l)
    s[1].v = 5
    print(l[1].v)


def main():
    print(count(1, 2, 3))
    refs(M(1), M(2))
    span_place()
