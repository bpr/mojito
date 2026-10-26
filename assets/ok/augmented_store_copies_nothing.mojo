# An augmented element store copies nothing, as at the pin: through a
# mutable-reference getter (`List`, a multi-index or slice `ref` getter, a
# generic `List[T]`) the in-place dunder runs on the element where it lies,
# and the operand is read by the dunder's convention. Through a value getter
# the getter's result is updated in place and the setter takes it (a `var`
# value), reads it (a `read` value), or converts it (an implicit conversion to
# its value type), so no move initializer runs.
trait Accum:
    def __iadd__(mut self, rhs: Self) raises:
        ...


struct C(Accum, Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        print("  copy", copy.v)
        self.v = copy.v

    def __init__(out self, *, deinit move: Self):
        print("  move", move.v)
        self.v = move.v

    def __deinit__(deinit self):
        print("  del", self.v)

    def __iadd__(mut self, rhs: Self) raises:
        if rhs.v < 0:
            raise Error("negative")
        self.v += rhs.v


struct Owned(Movable):
    var first: C

    def __init__(out self, var first: C):
        self.first = first^

    def __getitem__(self, i: Int) -> C:
        return self.first.copy()

    def __setitem__(mut self, i: Int, var value: C):
        self.first = value^


struct Lent(Movable):
    var first: C

    def __init__(out self, var first: C):
        self.first = first^

    def __getitem__(self, i: Int) -> C:
        return self.first.copy()

    def __setitem__(mut self, i: Int, value: C):
        self.first = value.copy()


struct Window(Movable):
    var cell: C

    def __init__(out self, var cell: C):
        self.cell = cell^

    def __getitem__(self, s: Slice) -> C:
        return self.cell.copy()

    def __setitem__(mut self, s: Slice, var value: C):
        self.cell = value^


struct Pair(Movable):
    var first: C

    def __init__(out self, var first: C):
        self.first = first^

    def __getitem__(self, i: Int, j: Int) -> C:
        return self.first.copy()

    def __setitem__(mut self, i: Int, j: Int, value: C):
        self.first = value.copy()


struct Total(Copyable):
    var v: Int

    @implicit
    def __init__(out self, c: C):
        print("  convert", c.v)
        self.v = c.v


struct Converting(Movable):
    var first: C
    var last: Int

    def __init__(out self, var first: C):
        self.first = first^
        self.last = 0

    def __getitem__(self, i: Int) -> C:
        return self.first.copy()

    def __setitem__(mut self, i: Int, value: Total):
        self.last = value.v


struct Grid(Movable):
    var cell: C

    def __init__(out self, var cell: C):
        self.cell = cell^

    def __getitem__(ref self, i: Int, j: Int) -> ref[self.cell] C:
        return self.cell

    def __getitem__(ref self, s: Slice) -> ref[self.cell] C:
        return self.cell


def accumulate[T: Accum & Copyable & Deinitable](mut items: List[T], x: T) raises:
    items[0] += x


def main() raises:
    var x = C(4)
    var l = List[C]()
    l.append(C(3))
    print("list element")
    l[0] += x
    print(l[0].v, x.v)
    print("generic element")
    accumulate(l, x)
    print(l[0].v, x.v)
    var g = Grid(C(10))
    print("multi-index reference getter")
    g[0, 0] += x
    print("slice reference getter")
    g[0:1] += x
    print(g.cell.v, x.v)
    print("transferred operand")
    l[0] += C(5)
    print("raising dunder")
    try:
        l[0] += C(-1)
    except e:
        print("caught", e)
    print(l[0].v)
    var o = Owned(C(1))
    print("value getter, var setter")
    o[0] += x
    print(o.first.v)
    var r = Lent(C(2))
    print("value getter, read setter")
    r[0] += x
    print(r.first.v)
    var w = Window(C(20))
    print("slice value getter")
    w[0:1] += x
    print(w.cell.v, x.v)
    var p = Pair(C(30))
    print("multi-index value getter")
    p[0, 0] += x
    print(p.first.v)
    var c = Converting(C(40))
    print("converting setter")
    c[0] += x
    print(c.last)
    var s = List[String]()
    s.append("ab")
    var t = String("  cd  ")
    s[0] += t.strip()
    print(s[0], t)
