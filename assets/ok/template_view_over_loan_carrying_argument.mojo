# A method returning a view of a field (`Span[Self.T, origin_of(self.items)]`)
# on an instance whose argument carries a loan (`Bag[Span[Int,
# origin_of(xs)]]`) derives from its checked template. The view's own origin
# slot is kept by template owner and bound to the instance's receiver; the
# slot the element type brings in names the clone's origin binder and is
# kept as it stands (`bind_struct_origins`).
struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def push(mut self, var value: Self.T):
        self.items.append(value^)

    def view(ref self) -> Span[Self.T, origin_of(self.items)]:
        return Span(self.items)


def main():
    var xs: List[Int] = [1, 2, 3]
    var b = Bag[Span[Int, origin_of(xs)]]()
    b.push(Span(xs))
    b.push(Span(xs))
    var v = b.view()
    var second = v[1]
    print(len(v), len(second), second[2])
    print(xs[0])
