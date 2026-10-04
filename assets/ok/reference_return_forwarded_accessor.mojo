# Forwarding a named accessor's reference result as the function's own stays
# within the declared origin, as the pin runs it: the accessor's `self` is its
# receiver, so `self.items.unsafe_get(index)` under
# `ref[origin_of(self.items)._get_owned_interior["element"]]` returns storage
# inside the receiver's element region, like the subscript `self.items[index]`.
struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def raw(
        ref self, index: Int
    ) -> ref[origin_of(self.items)._get_owned_interior["element"]] Self.T:
        return self.items.unsafe_get(index)

    def again(
        ref self, index: Int
    ) -> ref[origin_of(self.items)._get_owned_interior["element"]] Self.T:
        return self.raw(index)


def first(
    ref xs: List[Int],
) -> ref[origin_of(xs)._get_owned_interior["element"]] Int:
    return xs.unsafe_get(0)


def main():
    var shelf = Shelf[Int]()
    shelf.add(3)
    shelf.add(4)
    print(shelf.raw(0))
    print(shelf.again(1))
    var xs: List[Int] = [5, 6]
    print(first(xs))
