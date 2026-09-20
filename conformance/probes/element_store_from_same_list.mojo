# PROBE (divergence): an element of a `List` field stored from another element
# of the same list.
#
# The pinned Mojo copies `self.items[j]` out before it borrows `self.items`
# for the store, whatever the element type. Mojito accepts the `Int` instance
# and rejects the `String` one: "'self' is borrowed mutably and also used at
# the same call". The verdict depends on the instance's type, which is why a
# whole-element store stays outside the checked-template method grammar
# (`docs/notes/instantiation-from-template.md`, `SUBSCRIPT_STORES`).
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   mojo:   5 / y
#   mojito: rejects 'dup' instantiated for 'Shelf[String]'
#
# When fixed: promote to `assets/ok`.
struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def dup(mut self, i: Int, j: Int):
        self.items[i] = self.items[j]

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
    words.dup(0, 1)
    print(words.at(0))
