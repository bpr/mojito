# A struct construction in a parameter position is the application of the
# struct's `__init__`, the synthesized fieldwise one included, as at the
# pin: over a binder, in a signature, a field type, a body, and a struct
# parameter's default, by keyword, with a default filled in, through a module
# `comptime` alias, and of a generic, one-field register-passable, or library
# struct.
@fieldwise_init
struct Extent(ImplicitlyCopyable, Movable):
    var rows: Int
    var cols: Int


struct Sq(ImplicitlyCopyable, Movable):
    var rows: Int
    var cols: Int

    def __init__(out self, rows: Int, cols: Int = 1):
        self.rows = rows
        self.cols = cols * 2


@fieldwise_init
struct G[k: Int](ImplicitlyCopyable, Movable):
    var v: Int


@fieldwise_init
struct One(ImplicitlyCopyable, Movable, TrivialRegisterPassable):
    var v: Int


struct Tagged[e: Extent](ImplicitlyCopyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale

    def total(self) -> Int:
        return Self.e.rows * Self.e.cols * self.scale


struct TS[s: Sq](ImplicitlyCopyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale

    def total(self) -> Int:
        return Self.s.rows * Self.s.cols * self.scale


struct TG[g: G[2]](ImplicitlyCopyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale

    def total(self) -> Int:
        return Self.g.v * self.scale


struct TO[o: One](ImplicitlyCopyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale

    def total(self) -> Int:
        return Self.o.v * self.scale


struct Holder[n: Int]:
    var t: Tagged[Extent(Self.n, 2)]

    def __init__(out self):
        self.t = Tagged[Extent(Self.n, 2)](5)


comptime E = Extent(4, 4)


def area[e: Extent = Extent(2, 3)]() -> Int:
    return e.rows * e.cols


def over[n: Int]() -> Int:
    var t = Tagged[Extent(n, n + 1)](1)
    return t.total()


def canonical[n: Int]() -> Int:
    var a = Tagged[Extent(1 + n, n)](1)
    var c: Tagged[Extent(n + 1, n)] = a
    return c.total()


def signature[n: Int](t: Tagged[Extent(n, 2)]) -> Int:
    return t.total()


def local[n: Int]() -> Int:
    comptime e = Extent(n, n + 1)
    return Tagged[e](1).total()


def written[n: Int]() -> Int:
    return TS[Sq(n)](1).total()


def generic[n: Int]() -> Int:
    return TG[G[2](n)](1).total()


def held[n: Int]() -> Int:
    return TO[One(n)](2).total()


def main():
    print(over[3]())
    print(canonical[2]())
    print(signature[3](Tagged[Extent(3, 2)](1)))
    var d: Tagged[E] = Tagged[Extent(4, 4)](1)
    print(d.total())
    var k: Tagged[Extent(2, 3)] = Tagged[Extent(rows=2, cols=3)](1)
    print(k.total())
    print(local[3]())
    print(Holder[3]().t.total())
    var s: TS[Sq(3)] = TS[Sq(3, 1)](1)
    print(s.total(), written[2]())
    print(TG[G[2](5)](1).total(), generic[4]())
    print(TO[One(3)](2).total(), held[5]())
    comptime x = Extent(2, 3)
    print(x.rows, x.cols)
    print(area(), area[Extent(4, 4)]())
    comptime l = List[Int]()
    print(materialize[l]().__len__())
