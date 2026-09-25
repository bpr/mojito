# An element of a `List` field stored from another element of the same list.
# `__setitem__` takes its value by `var`, so the right-hand element is copied
# out before the store borrows the list mutably: the store is accepted whatever
# the element type, the stored copy is independent of its source, and a
# self-store (`i == j`) keeps the value.
struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def dup(mut self, i: Int, j: Int):
        self.items[i] = self.items[j]

    def set(mut self, i: Int, var value: Self.T):
        self.items[i] = value^

    def at(self, i: Int) -> Self.T:
        return self.items[i]


def main():
    var numbers = Shelf[Int]()
    numbers.add(4)
    numbers.add(5)
    numbers.dup(0, 1)
    print(numbers.at(0))
    var words = Shelf[String]()
    words.add("x")
    words.add("y")
    words.add("z")
    words.dup(0, 1)
    print(words.at(0), words.at(1))
    words.set(1, String("Q"))
    print(words.at(0), words.at(1))
    words.dup(2, 2)
    print(words.at(2))
