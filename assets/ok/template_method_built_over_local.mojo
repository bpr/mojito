# A per-instantiation method clone inherits its checked template's facts when
# the body binds a local whose type is built over the struct's parameter
# without being it (`docs/notes/instantiation-from-template.md`, class
# MethodBody): a bundled collection, a hand-written struct named or inferred at
# its construction, and a view a hand-written constructor's `ref` parameter
# infers. The declaration judges such a binding deletable from the type's own
# conformance, which realization asks again at the instance's arguments. Such a
# local is also a method call's receiver and `len`'s operand, as a field of
# `self` is.
struct View[T: Copyable & Deinitable, m: Bool, //, o: Origin[mut=m]]:
    var count: Int

    def __init__(out self, ref[Self.o] items: List[Self.T]):
        self.count = len(items)


struct Cell[T: Copyable & Deinitable](Copyable, Deinitable, Movable):
    var value: Self.T
    var tag: Int

    def __init__(out self, var value: Self.T, tag: Int):
        self.value = value^
        self.tag = tag


struct Shelf[T: Copyable & Deinitable]:
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def made(self) -> List[Self.T]:
        var result = List[Self.T]()
        return result^

    def count_of(self, var items: List[Self.T]) -> Int:
        return 3

    def handed(self) -> Int:
        var result = List[Self.T]()
        return self.count_of(result^)

    def wrapped(self, var value: Self.T) -> Cell[Self.T]:
        var cell = Cell[Self.T](value^, 2)
        return cell^

    def inferred(self, var value: Self.T) -> Cell[Self.T]:
        var cell = Cell(value^, 5)
        return cell^

    def unused(self) -> Int:
        var result = List[Self.T]()
        return 4

    def fresh(self) -> Int:
        var result = List[Self.T]()
        return len(result)

    def popped(self) -> Self.T:
        var result = self.items.copy()
        return result.pop()

    def cleared(self) -> Int:
        var result = self.items.copy()
        result.clear()
        return len(result)

    def viewed(self) -> Int:
        var view = View(self.items)
        return 1


def main():
    var numbers = Shelf[Int]()
    numbers.add(4)
    var words = Shelf[String]()
    words.add("x")
    print(len(numbers.made()), len(words.made()), numbers.handed(), words.handed())
    print(numbers.wrapped(7).tag, words.wrapped("y").value, numbers.inferred(8).tag, words.inferred("z").value)
    print(numbers.unused(), words.unused(), numbers.viewed(), words.viewed())
    print(numbers.fresh(), words.fresh(), numbers.popped(), words.popped(), numbers.cleared(), words.cleared())
