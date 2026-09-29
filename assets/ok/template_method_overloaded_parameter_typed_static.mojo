# A per-instantiation method clone inherits its checked template's facts when
# the body calls an overloaded static method of a generic struct on a spelled
# receiver (`Pair[Self.T].pick(v)`) whose members differ in a parameter of
# the struct's parameter type (`docs/notes/instantiation-from-template.md`,
# class MethodBody, feature `static_calls`). Each instance calls its clone
# of the template's member.


@fieldwise_init
struct Pair[T: Copyable & Deinitable](Copyable, Movable):
    var a: Self.T

    @staticmethod
    def pick(v: Self.T) -> Int:
        return 1

    @staticmethod
    def pick(v: Float64) -> Int:
        return 2


struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def picked(self) -> Int:
        return Pair[Self.T].pick(self.item)

    def given(self, v: Self.T) -> Int:
        return Pair[Self.T].pick(v)

    def literal(self) -> Int:
        return Pair[Self.T].pick(1.5)


def main():
    var numbers = Shelf[Int](3)
    var words = Shelf[String]("x")
    print(numbers.picked(), words.picked())
    print(numbers.given(4), words.given("y"))
    print(numbers.literal(), words.literal())
