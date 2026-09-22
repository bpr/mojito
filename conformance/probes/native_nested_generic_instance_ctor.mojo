# A struct instance whose argument is itself a generic instance
# (`Bag[List[Int]]`) runs on the VM but the native backend refuses its
# constructor: `Bag$mono$TList$u24$mono$u24$TInt$Int.__init__` "collides with
# an existing declaration". The per-instantiation `__init__` clone and the
# monomorphized constructor mangle to one symbol. Filed from the
# instantiation-from-template work (roadmap section 2); the pinned Mojo runs it.
struct Bag[T: Copyable & Deinitable](Movable):
    var item: Self.T
    var items: List[Self.T]

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.items = List[Self.T]()

    def count(self) -> Int:
        return len(self.items)


def main():
    var c = Bag[List[Int]](List[Int]())
    print(c.count())
