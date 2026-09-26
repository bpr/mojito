# A per-instantiation method clone inherits its checked template's facts when
# a method reads an element of a tuple-typed `var` local at a literal index
# (`docs/notes/instantiation-from-template.md`, class MethodBody). The index
# is the element's position, which no instance changes, and the element's
# type substitutes. Each instance reads the element through its own
# generated Tuple's accessor for that position, whether the template's tuple
# was closed (`Tuple[Int, Int]`) or built over the struct's parameter
# (`Tuple[Self.T, Int]`). `List`'s strided slice reads `slice.indices(n)`
# this way.
@fieldwise_init
struct Holder[T: ImplicitlyCopyable & Deinitable](
    Deinitable, ImplicitlyCopyable, Movable
):
    var value: Self.T
    var size: Int

    def bounds(self) -> Tuple[Int, Int]:
        return (self.size, self.size * 2)

    def tagged(self) -> Tuple[Self.T, Int]:
        return (self.value, self.size + 1)

    def span(self) -> Int:
        var pair = self.bounds()
        var low = pair[0]
        return pair[1] - low

    def tag(self) -> Int:
        var entry = self.tagged()
        var count = entry[1]
        return count * 10

    def width(self, stride: Slice) -> Int:
        var limits = stride.indices(self.size)
        var start = limits[0]
        var stop = limits[1]
        var step = limits[2]
        return (stop - start) // step


def main():
    var numbers = Holder[Int](7, 3)
    var words = Holder[String]("w", 5)
    print(numbers.span(), words.span())
    print(numbers.tag(), words.tag())
    var every_other = slice(0, 4, 2)
    print(numbers.width(every_other), words.width(every_other))
    var xs: List[Int] = [1, 2, 3, 4, 5]
    var ws: List[String] = ["a", "b", "c", "d"]
    var odd = xs[::2]
    var back = ws[::-1]
    print(len(odd), odd[0], odd[2], len(back), back[0], back[3])
