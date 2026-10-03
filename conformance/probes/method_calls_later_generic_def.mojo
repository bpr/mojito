# Probe: may a method call a generic `def` declared after its struct?
#
# The pin (2026-09-21) prints `1`. Mojito stops with "Undefined variable
# 'tally'"; the same `def` declared above the struct runs. `docs/roadmap.md`
# 3.116.
struct Shelf[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def size(self) -> Int:
        return tally(self.items)


def tally[T: Copyable & Deinitable](items: List[T]) -> Int:
    return len(items)


def main():
    var shelf = Shelf[Int]()
    shelf.items.append(8)
    print(shelf.size())
