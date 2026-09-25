# A per-instantiation method clone inherits its checked template's facts when
# an `if` or `while` tests a struct through `__bool__`
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `truthiness_conditions`). Whether a condition converts through `Bool(x)` is
# decided by its type, which each instance judges again: a field of `self`, a
# parameter, a `var` parameter tested by a `while`, and a `var` local all
# derive for an `Int` and a `String` instance. So do a `Bool(result)`
# conversion of a closed local and a reference call on that local, the shape
# of `List.index`.
@fieldwise_init
struct Gate(Boolable, Copyable, Movable):
    var open: Bool

    def __bool__(self) -> Bool:
        return self.open


struct Holder[T: Copyable & Deinitable & Equatable](Movable):
    var value: Self.T
    var gate: Gate

    def __init__(out self, var value: Self.T, open: Bool):
        self.value = value^
        self.gate = Gate(open)

    def pick(self, other: Self.T) -> Self.T:
        if self.gate:
            return self.value.copy()
        return other.copy()

    def through(self, gate: Gate, other: Self.T) -> Self.T:
        if gate:
            return self.value.copy()
        return other.copy()

    def countdown(self, var gate: Gate) -> Int:
        var steps = 0
        while gate:
            steps += 1
            if steps == 3:
                break
        return steps

    def find(self, value: Self.T) -> Optional[Int]:
        if self.value == value:
            return Optional[Int](0)
        return Optional[Int]()

    def found(self, value: Self.T) -> Bool:
        var result = self.find(value)
        if result:
            return True
        return False

    def index(self, value: Self.T) raises -> Int:
        var result = self.find(value)
        if not Bool(result):
            raise Error("not found")
        return result.value()


def main():
    var some_int = Holder[Int](1, True)
    var some_str = Holder[String](String("s"), False)
    print(some_int.pick(2), some_str.pick(String("o")))
    print(some_int.through(Gate(False), 9), some_str.through(Gate(True), String("q")))
    print(some_int.countdown(Gate(True)), some_str.countdown(Gate(False)))
    print(some_int.found(1), some_int.found(2), some_str.found(String("s")))
    try:
        print(some_int.index(1), some_str.index(String("s")))
        print(some_int.index(5))
    except e:
        print(e)
