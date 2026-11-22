# `Self` inside a member of a struct with an origin `mut=` binder holds the
# struct's own binder, which each instance substitutes: a type spelled over
# `Self.o` reads the instance's mutability.


struct Q[mut: Bool, //, o: Origin[mut=mut]]:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def own_slot(self) -> Bool:
        return Self.mut

    def span_slot(self) -> Bool:
        return Span[Int, Self.o].mut


def main():
    var xs = [1, 2, 3]
    var a = Q[origin_of(xs)](1)
    print(a.own_slot())  # True
    print(a.span_slot())  # True
    var b = Q[ImmOrigin(origin_of(xs))](1)
    print(b.own_slot())  # False
    print(b.span_slot())  # False
