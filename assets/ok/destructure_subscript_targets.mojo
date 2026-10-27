# A nominal subscript leaf of a destructuring assignment stores its element
# through `__setitem__`, as a plain subscript store does: each leaf's
# receiver and indices are evaluated before the right-hand side, then the
# setters run in order. A tuple display converts each element to its leaf's
# type, a `var` setter takes an owned element, and a getter returning a
# mutable reference is written through it.
struct Box(Movable):
    var a: Int
    var b: Int

    def __init__(out self):
        self.a = 0
        self.b = 0

    def __getitem__(self, i: Int) -> Int:
        return self.a if i == 0 else self.b

    def __setitem__(mut self, i: Int, value: Int):
        print("set", i, value)
        if i == 0:
            self.a = value
        else:
            self.b = value


struct Wide(Movable):
    var a: Float64

    def __init__(out self):
        self.a = 0

    def __getitem__(self, i: Int) -> Float64:
        return self.a

    def __setitem__(mut self, i: Int, val: Float64):
        self.a = val


struct Named(Movable):
    var s: String

    def __init__(out self):
        self.s = ""

    def __getitem__(self, i: Int) -> String:
        return self.s

    def __setitem__(mut self, i: Int, var v: String):
        self.s = v^


struct Cell(Movable):
    var a: Int

    def __init__(out self):
        self.a = 0

    def __getitem__(ref self, i: Int) -> ref[self.a] Int:
        return self.a


def idx(i: Int) -> Int:
    print("idx", i)
    return i


def pair() -> Tuple[Int, Int]:
    print("pair")
    return (3, 4)


def main() raises:
    var b = Box()
    (b[idx(0)], b[idx(1)]) = pair()
    print(b.a, b.b)
    var x = 0
    (x, b[1]) = (7, 8)
    print(x, b.b)
    var t = (9, 10)
    (b[0], b[1]) = t
    print(b.a, b.b, t[0])
    (b[0], (b[1], x)) = (1, (2, 3))
    print(b.a, b.b, x)

    var w = Wide()
    (w[0], x) = (5, 6)
    print(w.a, x)

    var xs: List[Int] = [0, 0, 0]
    (xs[0], (xs[1], xs[2])) = (1, (2, 3))
    print(xs[0], xs[1], xs[2])
    (xs[0], xs[0]) = (4, 5)
    print(xs[0])
    var pairs: List[Tuple[Int, Int]] = [(7, 8), (9, 10)]
    for (xs[0], xs[1]) in pairs:
        print(xs[0], xs[1])

    var names: List[String] = ["x", "y"]
    var u = (String("c"), String("d"))
    (names[1], names[0]) = u
    print(names[0], names[1], u[0])
    var d: Dict[String, Int] = {}
    (d["a"], d["b"]) = (1, 2)
    print(d["a"], d["b"])
    var n = Named()
    (n[0], x) = (String("q"), 3)
    print(n.s, x)

    var c = Cell()
    (c[0], x) = (11, 12)
    print(c.a, x)
