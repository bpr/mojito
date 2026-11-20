# A mutable span reaches an immutable one only through `Span`'s implicit
# view constructor: a `var` annotated with an `ImmOrigin(...)` span, a call
# to a function generic over an `ImmOrigin` binder, and a struct initializer
# taking an immutable span each convert, as at the pin.
struct V[o: ImmOrigin](Movable):
    var s: Span[Int, Self.o]

    def __init__(out self, s: Span[Int, Self.o]):
        self.s = s


def first[o: ImmOrigin](s: Span[Int, o]) -> Int:
    return s[0]


def main():
    var xs: List[Int] = [1, 2, 3]
    var m = Span(xs)
    var s: Span[Int, ImmOrigin(origin_of(xs))] = m
    var v = V(Span(xs))
    print(first(m), s[1], v.s[2])
