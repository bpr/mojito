# expect: access to 'xs' conflicts with live reference 'b'
# A template-served method that stores a copy of its argument publishes the
# stored type, and the call closes it with the receiver's arguments: the bag
# borrows `xs`, which the pointer type names, so `xs` is not written while
# the bag is still read.
struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def push(mut self, var value: Self.T):
        self.items.append(value^)

    def push_copy(mut self, value: Self.T):
        self.push(value.copy())


def main():
    var xs: List[Int] = [1, 2, 3]
    var b = Bag[Pointer[List[Int], ImmOrigin(origin_of(xs))]]()
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    b.push_copy(p)
    xs.append(9)
    print(len(b.items), b.items[0][][1])
