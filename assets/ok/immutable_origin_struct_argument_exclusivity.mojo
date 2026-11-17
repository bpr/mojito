# A value whose type names an `ImmOrigin(...)` origin, handed to a method of
# a receiver whose type names the same origin: the struct's origin slot
# carries the cast's `mut=False`, so the exclusivity check reads the value
# as an immutable loan beside the `mut self` receiver, as at the pin.
struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def push(mut self, var value: Self.T):
        self.items.append(value^)


def main():
    var xs: List[Int] = [1, 2, 3]
    var b = Bag[Span[Int, ImmOrigin(origin_of(xs))]]()
    var s: Span[Int, ImmOrigin(origin_of(xs))] = Span(xs)
    b.push(s)
    print(len(b.items), b.items[0][1])
