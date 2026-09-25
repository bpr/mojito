# A per-instantiation method clone inherits its checked template's facts when
# its body holds a list, set, or dict comprehension
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `COMPREHENSIONS`). Each generator clause records its iterator protocol at
# its iterable, which an instance selects again from its substituted type, and
# each binder is a local of the body declared from that protocol's binding
# plan: a borrowed element, an owned one, a closed scalar, and a nested
# comprehension all derive for an `Int` and a `String` instance.
struct Bag[T: ImplicitlyCopyable & Deinitable & Writable & Hashable & Equatable](
    Movable
):
    var items: List[Self.T]
    var counts: List[Int]

    def __init__(out self, var items: List[Self.T], var counts: List[Int]):
        self.items = items^
        self.counts = counts^

    def copies(self) -> List[Self.T]:
        return [x for x in self.items]

    def without(self, drop: Self.T) -> List[Self.T]:
        return [x for x in self.items if x != drop]

    def small(self, limit: Int) -> Int:
        var picked = [c * 2 for c in self.counts if c < limit]
        return len(picked)

    def distinct(self) -> Int:
        var seen = {x for x in self.items}
        return len(seen)

    def paired(self) -> Int:
        var table = {c: c + 1 for c in self.counts}
        return len(table)

    def product(self, other: List[Int]) -> List[Self.T]:
        return [x for x in self.items for n in other if n > 1]

    def grid(self) -> Int:
        var rows = [[x for x in self.items] for _ in self.twice()]
        return len(rows)

    def twice(self) -> List[Int]:
        return [len(self.items), len(self.items)]

    def drained(deinit self) -> List[Self.T]:
        return [x for x in self.items^]


def main():
    var ints: List[Int] = [1, 2, 2]
    var strings: List[String] = [String("x"), String("y")]
    var counts_a: List[Int] = [1, 5, 3]
    var counts_b: List[Int] = [4]
    var a = Bag[Int](ints^, counts_a^)
    var b = Bag[String](strings^, counts_b^)
    print(len(a.copies()), len(b.copies()))
    print(len(a.without(2)), len(b.without(String("x"))))
    print(a.small(4), b.small(5))
    print(a.distinct(), b.distinct())
    print(a.paired(), b.paired())
    var other_a: List[Int] = [1, 2, 3]
    var other_b: List[Int] = [2]
    print(len(a.product(other_a)), len(b.product(other_b)))
    print(a.grid(), b.grid())
    var left = a^.drained()
    var right = b^.drained()
    print(left[2], right[1])
