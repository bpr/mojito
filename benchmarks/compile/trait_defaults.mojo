# Trait defaults called through many conformers: a two-level refinement
# whose defaults call each other and the required method, reached from a
# generic driver at six plain structs and a generic struct at six element
# types. Some conformers override a default.


trait Shape:
    def area(self) -> Int:
        ...

    def name(self) -> String:
        return "shape"

    def double_area(self) -> Int:
        return self.area() * 2

    def describe(self) -> String:
        return self.name() + " " + String(self.double_area())


trait Scaled(Shape):
    def scale(self) -> Int:
        return 1

    def scaled_area(self) -> Int:
        return self.area() * self.scale()


@fieldwise_init
struct Square(Scaled):
    var side: Int

    def area(self) -> Int:
        return self.side * self.side


@fieldwise_init
struct Rect(Scaled):
    var w: Int
    var h: Int

    def area(self) -> Int:
        return self.w * self.h

    def name(self) -> String:
        return "rect"


@fieldwise_init
struct Tri(Scaled):
    var b: Int
    var h: Int

    def area(self) -> Int:
        return self.b * self.h // 2

    def scale(self) -> Int:
        return 3


@fieldwise_init
struct Dot(Scaled):
    var x: Int

    def area(self) -> Int:
        return 0


@fieldwise_init
struct Strip(Scaled):
    var n: Int

    def area(self) -> Int:
        return self.n


@fieldwise_init
struct Ring(Scaled):
    var outer: Int
    var inner: Int

    def area(self) -> Int:
        return self.outer * self.outer - self.inner * self.inner

    def name(self) -> String:
        return "ring"

    def scale(self) -> Int:
        return 2


@fieldwise_init
struct Tile[T: Copyable & Movable & Deinitable](Scaled):
    var size: Int
    var tag: Self.T

    def area(self) -> Int:
        return self.size


def report[S: Scaled](s: S) -> String:
    return s.describe() + " " + String(s.scaled_area())


def main():
    print(report(Square(2)))
    print(report(Rect(2, 3)))
    print(report(Tri(4, 3)))
    print(report(Dot(1)))
    print(report(Strip(5)))
    print(report(Ring(3, 1)))
    print(report(Tile[Int](6, 0)))
    print(report(Tile[String](7, String("t"))))
    print(report(Tile[Bool](8, True)))
    print(report(Tile[Float64](9, 0.5)))
    print(report(Tile[UInt8](10, UInt8(1))))
    print(report(Tile[List[Int]](11, [1, 2])))
