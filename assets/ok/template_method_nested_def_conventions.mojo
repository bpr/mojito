# A per-instantiation method clone inherits its checked template's facts when
# the body declares a nested `def` with a parameter convention, a default, or
# `raises` (`docs/notes/instantiation-from-template.md`, class MethodBody,
# feature `NESTED_DEFS`): a `var`, `mut`, or `ref` parameter, a defaulted one,
# and a raising nested `def`. Each call's arguments are judged against the
# parameter's recorded convention, and a raising call keeps its effect.


struct Shelf[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]
    var bias: Int

    def __init__(out self, bias: Int):
        self.items = List[Self.T]()
        self.bias = bias

    def owned(self, item: Self.T) -> Self.T:
        def keep(var x: Self.T) -> Self.T:
            return x^

        return keep(item.copy())

    def mutated(self, k: Int) -> Int:
        var total = k

        def bump(mut x: Int, y: Int):
            x += y

        bump(total, 3)
        return total

    def referenced(self, k: Int) -> Int:
        var total = k

        def twice(ref x: Int) -> Int:
            return x * 2

        return twice(total)

    def defaulted(self, k: Int) -> Int:
        def scaled(x: Int, by: Int = 3) -> Int:
            return x * by + 1

        return scaled(k) + scaled(k, 2) + scaled(by=4, x=k)

    def raising(self, k: Int) raises -> Int:
        def check(x: Int) raises -> Int:
            if x < 0:
                raise Error("negative")
            return x + 1

        return check(k)

    def grown(self, item: Self.T) -> Int:
        var list = self.items.copy()

        def push(mut xs: List[Self.T], x: Self.T):
            xs.append(x.copy())

        push(list, item)
        return len(list)

    def viewed(self, item: Self.T) -> Self.T:
        var kept = item.copy()

        def peek(ref x: Self.T) -> Self.T:
            return x.copy()

        return peek(kept)

    def handed(self, item: Self.T) -> Self.T:
        var kept = item.copy()

        def keep(var x: Self.T) -> Self.T:
            return x^

        return keep(kept^)

    def caught(self, k: Int) -> Int:
        def check(x: Int, floor: Int = 0) raises -> Int:
            if x < floor:
                raise Error("below")
            return x

        try:
            return check(k, 10)
        except:
            return -k


def main() raises:
    var ints = Shelf[Int](1)
    ints.items.append(1)
    var words = Shelf[String](2)
    words.items.append("x")
    words.items.append("y")
    print(ints.owned(5), words.owned("w"))
    print(ints.mutated(3), words.mutated(4))
    print(ints.referenced(3), words.referenced(4))
    print(ints.defaulted(3), words.defaulted(4))
    print(ints.raising(3), words.raising(4))
    print(ints.grown(3), words.grown("g"))
    print(ints.viewed(3), words.viewed("v"))
    print(ints.handed(3), words.handed("h"))
    print(ints.caught(3), words.caught(12))
