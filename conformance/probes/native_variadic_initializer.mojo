# A struct whose initializer collects `var *values` runs on the VM but the
# native backend refuses its constructor at every instance, `Bag[Int]`
# included: `Bag$mono$TInt.__init__` "collides with an existing
# declaration". Filed from the variadic-initializer derivation (roadmap
# section 2); the pinned Mojo prints "2".
struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self, var *values: Self.T):
        self.items = List[Self.T]()
        for value in values:
            self.items.append(value.copy())


def main():
    var b = Bag(1, 2)
    print(len(b.items))
