# A callee declared `@__unsafe_nested_origins_read_only` only reads the
# origins its argument types carry, so two arguments carrying one mutable
# origin do not alias: a user method and a user function so declared, and
# the bundled `List.append`.
struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    @__unsafe_nested_origins_read_only
    def push(mut self, var value: Self.T):
        self.items.append(value^)

    def count(self) -> Int:
        return len(self.items)


@__unsafe_nested_origins_read_only
def pick[T: Copyable](items: List[T], default: T) -> T:
    if len(items) > 0:
        return items[0].copy()
    return default.copy()


def main():
    var xs: List[Int] = [1, 2, 3]
    var b = Bag[Span[Int, origin_of(xs)]]()
    b.push(Span(xs))
    b.push(Span(xs))
    var l = List[Span[Int, origin_of(xs)]]()
    l.append(Span(xs))
    print(b.count(), len(l), pick(l, Span(xs))[2])
    xs[0] = 7
    print(xs[0])
