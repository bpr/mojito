# A trait default holding a `comptime if` is a method of each conformer that
# inherits it, so its condition is decided where the conformer's own methods'
# are: a literal, a reflection query over a named struct, and a default
# reached through a refined trait, on a plain and a generic conformer, called
# directly and through a trait bound.
@fieldwise_init
struct Point:
    var x: Int
    var y: Int


trait Shaped:
    def corners(self) -> Int:
        comptime if True:
            return 4
        else:
            return 0

    def axes(self) -> Int:
        comptime if reflect[Point].field_count() == 2:
            return reflect[Point].field_count()
        else:
            return -1


trait Named(Shaped):
    def label(self) -> String:
        comptime if reflect[Point].is_struct():
            return "shaped"
        else:
            return "unshaped"


@fieldwise_init
struct Square(Named):
    var side: Int


@fieldwise_init
struct Tile[T: Copyable & Deinitable](Named):
    var payload: Self.T


def describe[S: Named](s: S) -> String:
    return s.label() + " " + String(s.corners() * s.axes())


def main():
    var square = Square(3)
    print(square.corners(), square.axes(), square.label())
    var tile = Tile[String]("t")
    print(tile.corners(), tile.axes(), tile.label())
    print(describe(square))
    print(describe(Tile[Int](1)))
