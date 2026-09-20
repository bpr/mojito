# A method that reads a reference result by value derives from its checked
# template only because the template marked the read copyable. Where the
# struct's bound grants no implicit copy the template marks nothing and is
# rejected itself, so no instance is ever left to owe the copy.
# expect: cannot be implicitly copied
struct Rack[T: Copyable & Deinitable]:
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def first(self) -> Self.T:
        return self.items[0]


def main():
    var counts = Rack[Int]()
    counts.add(8)
    print(counts.first())
