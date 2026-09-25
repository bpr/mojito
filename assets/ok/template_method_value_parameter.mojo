# A generic struct's methods are inferred once, with the struct's parameters
# symbolic, and the checked facts are reused in every later pass
# (`docs/notes/instantiation-from-template.md`, class MethodBody). A struct
# with a scalar value parameter (`Grid[T, rows: Int]`, as `Array`) or an
# origin parameter (`Span`) is never cloned whole, so its methods run on the
# erased path: a `Self.rows` read is a runtime read of the reified value, and
# a pointer field whose provenance is the struct's own origin names no
# checker-local place, so both keep the template's facts as they are. A
# receiver origin naming the method's own binder (`ref [o] self`) is a
# signature fact, so each per-instantiation clone of `hits_of` derives.
from std.collections.array import Array


struct Grid[T: Copyable & Deinitable, rows: Int](Movable):
    var cells: List[Self.T]

    def __init__(out self, var cells: List[Self.T]):
        self.cells = cells^

    def count(self) -> Int:
        return Self.rows

    def total(self) -> Int:
        return Self.rows * len(self.cells)

    def full(self) -> Bool:
        return len(self.cells) >= Self.rows


struct Cell[T: Copyable & Deinitable](Movable):
    var item: Self.T
    var hits: Int

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.hits = 1

    def hits_of[o: Origin](ref [o] self) -> Int:
        return self.hits + 1


def main():
    var xs: List[Int] = [1, 2, 3]
    var s = Span[Int, origin_of(xs)](xs)
    print(s[1], len(s))
    var ys: List[String] = ["a", "b"]
    var t = Span[String, origin_of(ys)](ys)
    print(t[0], len(t))
    var a = Array[Int, 3](fill=0)
    a[1] = 5
    print(a[1], a.unsafe_get(1), len(a))
    var b = Array[String, 2](fill="z")
    print(b[0], len(b))
    var gl: List[Int] = [1, 2]
    var hl: List[String] = ["q", "r", "s"]
    var g = Grid[Int, 4](gl^)
    var h = Grid[String, 2](hl^)
    print(g.count(), g.total(), g.full(), h.count(), h.total(), h.full())
    var c = Cell[Int](3)
    var d = Cell[String]("s")
    print(c.hits_of(), d.hits_of())
