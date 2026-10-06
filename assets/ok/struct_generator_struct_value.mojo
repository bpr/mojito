# A struct keyed on a struct-typed value is a generator: its members are
# checked once with the value symbolic, a field of the value reads in a
# `comptime if`, a runtime expression, and a lane width, and each instance
# folds them from its own frozen value.
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

    def wide(self) -> Int:
        comptime if Self.e.cols > 2:
            return 100 * self.scale
        return Self.e.rows

    def lanes(self) -> Int:
        var v = SIMD[DType.int32, Self.e.rows](1)
        return Int(v.reduce_add())


def rows_of[e: Extent]() -> Int:
    return e.rows * 10 + e.cols


def main():
    var a = Tagged[Extent(2, 3)](10)
    print(a.wide(), a.lanes())
    var b = Tagged[Extent(4, 1)](10)
    print(b.wide(), b.lanes())
    var c = Tagged[Extent.square(8)](1)
    print(c.wide(), c.lanes())
    print(rows_of[Extent(2, 3)](), rows_of[Extent.square(5)]())
