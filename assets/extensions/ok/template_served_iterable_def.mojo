# A generic `def` over an `Iterable` parameter iterates the collection in
# place: `for item in items` borrows a named source of a parameter type as it
# borrows a nominal one, so the template destroys nothing it does not own.
# The elaborator instantiates the template where a call binds a loan-carrying
# argument (`List[Span[Int, origin_of(xs)]]`) or reaches it from a
# template-served method (`Shelf[Int].first`), and the iterator step it
# resolves copies the element out of the reference the step returns.
# `C.Element` on an `Iterable` bound and `std.algorithms` are Mojito's own.
from std.algorithms import first_or


struct Shelf[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def first(self, fallback: Self.T) -> Self.T:
        return first_or(self.items, fallback)


def pick[C: Iterable](items: C, fallback: C.Element) -> C.Element:
    for item in items:
        return item.copy()
    return fallback.copy()


def picked(xs: List[Int]):
    var spans = List[Span[Int, origin_of(xs)]]()
    spans.append(Span(xs))
    var fallback = Span(xs)
    print(len(pick(spans, fallback)), first_or(spans, fallback)[2])
    print(len(spans), len(fallback))


def pointed():
    var xs: List[Int] = [4, 5, 6]
    var pointers = List[Pointer[List[Int], ImmOrigin(origin_of(xs))]]()
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    pointers.append(p)
    var q = pick(pointers, p)
    print(q[][1], len(pointers))


def shelved():
    var shelf = Shelf[Int]()
    print(shelf.first(7))
    shelf.items.append(8)
    print(shelf.first(7))


def main():
    picked([1, 2, 3])
    pointed()
    shelved()
