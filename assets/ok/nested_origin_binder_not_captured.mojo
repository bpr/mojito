# A call's own origin binder inside a type argument (`o` in
# `Span[Span[Int, o], _]`) is not the receiver struct's origin slot, even when
# the two share a declaration index: reading an element of the outer view
# yields a `Span[Int, o]`, which a `List[Span[Int, o]]` accepts.
def gather[
    m: Bool, p: Origin[mut=m], o: Origin[mut=m], //
](anchor: Span[Int, p], views: Span[Span[Int, o], _], mut out: List[Span[Int, o]]):
    out.append(views[0].copy())


def count(xs: List[Int], ys: List[Int]) -> Int:
    var inner = List[Span[Int, origin_of(xs)]]()
    inner.append(Span(xs))
    var out = List[Span[Int, origin_of(xs)]]()
    gather(Span(ys), Span(inner), out)
    return len(out) * 10 + out[0][0]


def main():
    var xs = List[Int]()
    xs.append(7)
    var ys = List[Int]()
    print(count(xs, ys))
