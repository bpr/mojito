# A per-instantiation method clone inherits its checked template's facts when
# the body calls a static method of a non-generic struct on its type, spelled
# (`Color.pick(n)`) or through a leading-dot contextual root (`.of(n)`)
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `static_calls`). The struct, the member the closed arguments select, and
# the base the expected type resolves are the same under every instance.


@fieldwise_init
struct Color(Copyable, Movable):
    var value: Int

    @staticmethod
    def red() -> Color:
        return Color(1)

    @staticmethod
    def of(v: Int) -> Color:
        return Color(v)

    @staticmethod
    def pick(v: Int) -> Int:
        return v * 2

    @staticmethod
    def pick(v: Float64) -> Int:
        return 7


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def first(self) -> Color:
        return .red()

    def shade(self, n: Int) -> Color:
        var c: Color = .of(n + 1)
        return c^

    def spelled(self) -> Color:
        return Color.red()

    def picked(self, n: Int) -> Int:
        return Color.pick(n) + Color.pick(1.5)


def main():
    var ints = Shelf[Int](3)
    var words = Shelf[String]("x")
    print(ints.first().value, words.first().value)
    print(ints.shade(4).value, words.shade(5).value)
    print(ints.spelled().value, words.spelled().value)
    print(ints.picked(4), words.picked(6))
