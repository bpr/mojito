# A per-instantiation method clone inherits its checked template's facts when
# its body calls a method on a subscripted element (`docs/notes/
# instantiation-from-template.md`, class MethodBody): a requirement of a bare
# parameter's bound on `self.items[j]`, read, mutating, or copying it, and one
# on an element of a read parameter's field (`other.entries[i].key.copy()`),
# with the built-in `len` of that field. Each instance re-selects the method
# on its own element type, borrowing the element through the getter's
# reference. The bundled `Dict.update` has the same shape.
trait Bumpable:
    def bump(mut self):
        ...

    def get(self) -> Int:
        ...


@fieldwise_init
struct Counter(Bumpable, Copyable, Movable):
    var n: Int

    def bump(mut self):
        self.n += 1

    def get(self) -> Int:
        return self.n


@fieldwise_init
struct Tally(Bumpable, Copyable, Movable):
    var n: Int

    def bump(mut self):
        self.n += 10

    def get(self) -> Int:
        return self.n * 2


struct Holder[T: Bumpable & Copyable & Movable & Deinitable](Copyable, Movable):
    var items: List[Self.T]

    def __init__(out self, var items: List[Self.T]):
        self.items = items^

    def dup(mut self, i: Int, j: Int):
        self.items[i] = self.items[j].copy()

    def local_elem(self, i: Int) -> Self.T:
        var x = self.items[i].copy()
        return x^

    def bump_at(mut self, i: Int):
        self.items[i].bump()

    def get_at(self, i: Int) -> Int:
        return self.items[i].get()

    def take(self, other: Holder[Self.T], i: Int) -> Self.T:
        return other.items[i].copy()


@fieldwise_init
struct Entry[K: Copyable & Movable & Deinitable](Copyable, Movable):
    var key: Self.K
    var n: Int


struct Table[K: Copyable & Movable & Deinitable](Copyable, Movable):
    var entries: List[Entry[Self.K]]

    def __init__(out self, var entries: List[Entry[Self.K]]):
        self.entries = entries^

    def other_key(self, other: Table[Self.K], i: Int) -> Self.K:
        return other.entries[i].key.copy()

    def total(self, other: Table[Self.K]) -> Int:
        var i = 0
        var sum = 0
        while i < len(other.entries):
            sum += other.entries[i].n
            i += 1
        return sum


def main() raises:
    var lc: List[Counter] = [Counter(1), Counter(2)]
    var c = Holder[Counter](lc^)
    c.dup(0, 1)
    c.bump_at(1)
    print(c.get_at(0), c.get_at(1), c.local_elem(1).n, c.take(c, 0).n)
    var lt: List[Tally] = [Tally(1), Tally(2)]
    var t = Holder[Tally](lt^)
    t.dup(1, 0)
    t.bump_at(0)
    print(t.get_at(0), t.get_at(1), t.local_elem(0).n, t.take(t, 1).n)
    var ei: List[Entry[Int]] = [Entry[Int](5, 1), Entry[Int](6, 2)]
    var a = Table[Int](ei^)
    var es: List[Entry[String]] = [Entry[String]("k", 3)]
    var b = Table[String](es^)
    print(a.other_key(a, 1), a.total(a), b.other_key(b, 0), b.total(b))
    var d: Dict[String, Int] = {"a": 1}
    var e: Dict[String, Int] = {"b": 2, "a": 3}
    d.update(e)
    var f: Dict[Int, String] = {1: "x"}
    var g: Dict[Int, String] = {2: "y"}
    f.update(g)
    print(len(d), d["a"], d["b"], len(f), f[2])
