# A struct-valued bracket argument that calls a function is that
# application until the elaborator below the check evaluates it: in a
# signature, a field type, a body, and over a binder, and through a
# `comptime` alias, which names the same type.
@fieldwise_init
struct Extent(ImplicitlyCopyable, Movable):
    var rows: Int
    var cols: Int

    @staticmethod
    def square(n: Int) -> Extent:
        return Extent(n, n)


struct Tagged[e: Extent](ImplicitlyCopyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale

    def total(self) -> Int:
        return Self.e.rows * Self.e.cols * self.scale


struct Keyed[e: Extent, k: Int]:
    @staticmethod
    def show():
        print(Self.e.rows, Self.k, sep="")


struct Holder:
    var t: Tagged[Extent.square(2)]

    def __init__(out self):
        self.t = Tagged[Extent.square(2)](5)


comptime Four = Extent.square(4)


def use(t: Tagged[Extent.square(3)]) -> Int:
    return t.total()


def rows_of[e: Extent]() -> Int:
    return e.rows * 10 + e.cols


def over[n: Int]() -> Int:
    var t = Tagged[Extent.square(n)](1)
    return t.total()


def main():
    Keyed[k=3, Extent.square(2)].show()
    print(use(Tagged[Extent.square(3)](1)))
    print(rows_of[Extent.square(5)]())
    print(Holder().t.total())
    print(over[3]())
    var d: Tagged[Four] = Tagged[Extent.square(4)](1)
    print(d.total())
    comptime Local = Extent.square(4)
    var e: Tagged[Local] = d
    print(e.total())
