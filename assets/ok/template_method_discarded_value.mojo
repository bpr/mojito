# A per-instantiation method clone inherits its checked template's facts when
# a method discards a whole value of the struct's parameter type with `_ =`
# (`docs/notes/instantiation-from-template.md`, class MethodBody). The discard
# copies a place as a `var` binding would, or takes a transferred local or a
# call result, and each instance destroys the value at its own type as it
# does any temporary.
@fieldwise_init
struct Holder[T: ImplicitlyCopyable & Deinitable](
    Deinitable, ImplicitlyCopyable, Movable
):
    var value: Self.T
    var size: Int

    def tagged(self) -> Tuple[Self.T, Int]:
        return (self.value, self.size + 1)

    def head(self) -> Int:
        var entry = self.tagged()
        var first = entry[0]
        _ = first
        return self.size

    def field(self) -> Int:
        var first = self.value
        _ = first
        _ = self.value
        return self.size + 1

    def param(self, x: Self.T) -> Int:
        _ = x
        return self.size + 2

    def moved(self) -> Int:
        var first = self.value
        _ = first^
        return self.size + 3


def main():
    var numbers = Holder[Int](7, 3)
    var words = Holder[String]("w", 5)
    print(numbers.head(), words.head())
    print(numbers.field(), words.field())
    print(numbers.param(1), words.param("q"))
    print(numbers.moved(), words.moved())
