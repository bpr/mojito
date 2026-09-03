# A borrowing view built inline as a parameterized-static argument stays
# anchored across the call it feeds — for both static-receiver parses: the
# TypeApply spelling (two struct arguments) and the single-argument spelling
# that parses as a value subscript. The hidden argument slot's loan keeps the
# view's source alive through the call even though it has no later use.


struct Pair[A: Copyable & Movable, B: Copyable & Movable]:
    var first: Self.A
    var second: Self.B

    @staticmethod
    def sum(view: Span[Int, _]) -> Int:
        var total = 0
        for x in view:
            total += x
        return total


struct Tally[T: Copyable & Movable]:
    var seed: Self.T

    @staticmethod
    def total(view: Span[Int, _]) -> Int:
        var total = 0
        for x in view:
            total += x
        return total


def main():
    var a: List[Int] = [1, 2, 3]
    print(Pair[Int, String].sum(Span(a)))
    var b: List[Int] = [4, 5]
    print(Tally[Int].total(Span(b)))
