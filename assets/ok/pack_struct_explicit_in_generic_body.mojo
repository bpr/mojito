# An explicit application of a variadic struct inside a generic body is
# matched against the template's constructor with the pack bound from its
# arguments: a `*args: *Self.Ts` collector takes one argument per bound
# element, whatever their types, and a valid construction inside a
# `comptime if` arm is accepted from the template as the pinned Mojo accepts
# it.
struct Bag[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def size(self) -> Int:
        return len(self.storage)


def make[T: Copyable & Movable & Deinitable](var x: T) -> Bag[T, String]:
    return Bag[T, String](x^, "tail")


def choose[n: Int]() -> Int:
    comptime if n == 0:
        return 1
    else:
        var b = Bag[Int, Bool](1, True)
        var t = Tuple[Float64, String](2, "two")
        return b.size() + len(t)


def main():
    var b = make(3)
    print(b.storage[0], b.storage[1])
    var s = make(String("s"))
    print(s.storage[0], s.storage[1], s.size())
    print(choose[0]())
    print(choose[1]())
