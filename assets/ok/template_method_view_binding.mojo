# A per-instantiation method clone inherits its checked template's facts when
# the body binds an annotated view whose origin is left to inference, or
# passes a place to a sibling's view parameter
# (`docs/notes/instantiation-from-template.md`, class MethodBody): the
# instance re-selects the view conversion at its own types, and the borrow of
# the source place is the template's, read or mutable alike.
struct Shelf[T: Copyable & Deinitable](Deinitable, Movable):
    var items: List[Self.T]

    def __init__(out self, var first: Self.T, var second: Self.T):
        self.items = List[Self.T]()
        self.items.append(first^)
        self.items.append(second^)

    def viewed(self) -> Int:
        var span: Span[Self.T, _] = self.items
        return len(span)

    def counted(mut self) -> Int:
        var span: Span[Self.T, _] = self.items
        var total = 0
        for _ in span:
            total += 1
        return total

    def grown(mut self, var value: Self.T) -> Int:
        self.items.append(value^)
        var span: Span[Self.T, _] = self.items
        return len(span)

    def measure(self, span: Span[Self.T, _]) -> Int:
        return len(span)

    def measured(self) -> Int:
        return self.measure(self.items)


def main():
    var numbers = Shelf[Int](4, 5)
    var words = Shelf[String]("x", "y")
    print(numbers.viewed(), words.viewed())
    print(numbers.counted(), words.counted())
    print(numbers.grown(6), words.grown("z"))
    print(numbers.measured(), words.measured())
