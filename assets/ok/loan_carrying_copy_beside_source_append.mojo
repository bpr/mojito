# A copy of a loan-carrying view element, taken through a `Span` of its
# list, beside an append to that list. The copy keeps the list alive but
# borrows nothing exclusively, so the append runs, as at the pin: `2 3`.
def g(xs: List[Int]):
    var l: List[Span[Int, origin_of(xs)]] = [Span(xs)]
    var v = Span(l)
    var w = v[0].copy()
    l.append(Span(xs))
    print(w[1], w[2])


def main():
    g([1, 2, 3])
