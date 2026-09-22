# A per-instantiation method clone inherits its checked template's facts when
# the body constructs a struct (`docs/notes/instantiation-from-template.md`,
# class MethodBody, feature `constructions`): a `copy:` construction of the
# struct's own type, a fieldwise construction, a bundled collection over the
# struct's parameter, and a hand-written constructor family whose selected
# member the instance retargets to its own `__init__` clone. Every argument is
# a closed scalar, a whole value, or the `copy:` of a named place, so the
# constructor the template selected binds each argument exactly under every
# instance, and the constructed type substitutes.
@fieldwise_init
struct Pair[T: Copyable & Deinitable](Movable):
    var item: Self.T
    var n: Int


struct Tagged[T: Copyable & Deinitable](Movable):
    var item: Self.T
    var tag: Int

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.tag = 0

    def __init__(out self, var item: Self.T, tag: Int):
        self.item = item^
        self.tag = tag


struct Box[T: Copyable & Deinitable](Copyable, Movable):
    var item: Self.T
    var n: Int
    var spare: List[Self.T]

    def __init__(out self, var item: Self.T, n: Int):
        self.item = item^
        self.n = n
        self.spare = List[Self.T]()

    def __init__(out self, *, copy: Self):
        self.item = copy.item.copy()
        self.n = copy.n
        self.spare = copy.spare.copy()

    def copy(self) -> Self:
        return Box[Self.T](copy=self)

    def empty(self) -> List[Self.T]:
        return List[Self.T]()

    def some(self) -> Optional[Self.T]:
        return Optional[Self.T](self.item.copy())

    def pair(self) -> Pair[Self.T]:
        return Pair[Self.T](self.item.copy(), self.n)

    def tagged(self) -> Tagged[Self.T]:
        return Tagged[Self.T](self.item.copy())

    def tagged_as(self, tag: Int) -> Tagged[Self.T]:
        return Tagged[Self.T](self.item.copy(), tag)

    def reset(mut self):
        self.spare = List[Self.T]()

    def keep(mut self, var item: Self.T):
        self.spare = List[Self.T]()
        self.spare.append(item^)

    def count(self) -> Int:
        return len(self.spare)


def main():
    var a = Box[Int](1, 2)
    var b = Box[String](String("x"), 3)
    var c = a.copy()
    var d = b.copy()
    print(c.item, c.n, d.item, d.n, len(a.empty()), len(b.empty()))
    print(a.some().value(), b.some().value(), a.pair().item, a.pair().n, b.pair().item)
    print(a.tagged().item, a.tagged().tag, b.tagged_as(7).item, b.tagged_as(7).tag)
    a.keep(5)
    b.keep(String("y"))
    print(a.count(), b.count(), a.spare[0], b.spare[0])
    a.reset()
    b.reset()
    print(a.count(), b.count())
