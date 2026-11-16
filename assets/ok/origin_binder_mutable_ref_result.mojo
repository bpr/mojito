# A `ref[o]` result over a mut-parametric binder `o: Origin` takes the
# mutability the call bound `o` with: a pointer to a `var`, or a span over
# one, yields a reference the caller may assign through.
def at[o: Origin](r: Pointer[Int, o]) -> ref[o] Int:
    return r[]


def at_span[o: Origin](s: Span[Int, o]) -> ref[o] Int:
    return s[0]


def main():
    var x = 1
    at(Pointer(to=x)) = 5
    print(x)
    var l = List[Int]()
    l.append(7)
    var s = Span(l)
    at_span(s) = 6
    print(l[0])
