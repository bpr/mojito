# An instance of a user template whose argument carries a loan
# (`Bag[Span[Int, origin_of(xs)]]`) clones its methods: each origin slot of the
# argument becomes an origin binder the clone declares and infers per call
# from its receiver and arguments, so one clone serves every origin of that
# shape. The clone derives its facts from the checked template, replaying the
# template's transfers on its own bindings: the `var value` moved into
# `self.items` keeps its source, since the binding may still carry a loan
# (`docs/notes/instantiation-from-template.md`, obligation 14). A generic
# `def` over the same argument clones the same way.
struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def push(mut self, var value: Self.T):
        self.items.append(value^)

    def push_held(mut self, var value: Self.T):
        var held = value^
        self.items.append(held^)

    def get(self, index: Int) -> Self.T:
        return self.items[index].copy()

    def count(self) -> Int:
        return len(self.items)


struct Shelf[T: Copyable & Deinitable](Movable):
    var bag: Bag[Self.T]

    def __init__(out self):
        self.bag = Bag[Self.T]()

    def put(mut self, var value: Self.T):
        self.bag.push(value^)


def first[T: Copyable](items: List[T]) -> T:
    return items[0].copy()


def total(xs: List[Int], ys: List[Int]) -> Int:
    var from_xs = Bag[Span[Int, origin_of(xs)]]()
    var view = Span(xs)
    from_xs.push(view)
    from_xs.push_held(Span(xs))
    var from_ys = Bag[Span[Int, origin_of(ys)]]()
    from_ys.push(Span(ys))
    var shelf = Shelf[Span[Int, origin_of(xs)]]()
    shelf.put(view)
    var picked = from_xs.get(1)
    var head = first(from_ys.items)
    return (
        from_xs.count() * 1000
        + from_ys.count() * 100
        + shelf.bag.count() * 10
        + picked[1]
        + head[0]
        + len(xs)
    )


def main():
    var xs = List[Int]()
    xs.append(3)
    xs.append(4)
    var ys = List[Int]()
    ys.append(5)
    print(total(xs, ys))
