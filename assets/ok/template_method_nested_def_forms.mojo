# A per-instantiation method clone inherits its checked template's facts when
# the body declares a nested `def` beyond closed scalars
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `NESTED_DEFS`): a parameter or result of the struct parameter's type, a
# capture-all default, a `ref` or transferred capture, a capture of `self`, and
# a nested `def` inside another. The signature substitutes in the recipe, each
# parameter's deletability and each owned capture's capability are judged at
# the instance's types, and every capture is rooted at a binding the instance
# maps to its own.


struct Shelf[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]
    var bias: Int

    def __init__(out self, bias: Int):
        self.items = List[Self.T]()
        self.bias = bias

    def echoed(self, item: Self.T) -> Self.T:
        def same(x: Self.T) -> Self.T:
            return x.copy()

        return same(item)

    def fresh(self, item: Self.T) -> Self.T:
        def same(x: Self.T) -> Self.T:
            return x.copy()

        var got = same(item.copy())
        return got^

    def picked(self, item: Self.T) -> Self.T:
        def pick() {imm item} -> Self.T:
            return item.copy()

        return pick()

    def counted(self) -> Int:
        def count(xs: List[Self.T]) -> Int:
            return len(xs)

        return count(self.items) + count(self.items.copy())

    def defaulted(self, k: Int) -> Int:
        var base = len(self.items)

        def times(x: Int) {imm} -> Int:
            return base * x + k

        return times(2)

    def copied(self, k: Int) -> Int:
        var seed = len(self.items)

        def probe(x: Int) {var} -> Int:
            return x + seed + k

        seed = 50
        return probe(k) + seed

    def referenced(self, k: Int) -> Int:
        var total = k

        def bump(x: Int) {ref total}:
            total += x

        bump(3)
        return total

    def moved(self, var item: Self.T, k: Int) -> Int:
        def keep(x: Int) {var item^} -> Int:
            return x + 1

        return keep(k)

    def selfish(self, k: Int) -> Int:
        def peek(x: Int) {imm self} -> Int:
            return x + self.bias + len(self.items)

        return peek(k)

    def bumped(mut self, k: Int) -> Int:
        def add(x: Int) {mut}:
            self.bias += x

        def again(x: Int) {mut self}:
            self.bias += x * 2

        add(k)
        again(k)
        return self.bias

    def layered(self, k: Int) -> Int:
        def outer(x: Int) -> Int:
            def inner(y: Int) -> Int:
                return y * 2

            return inner(x) + 1

        return outer(k)

    def deep(self, k: Int) -> Int:
        var total = k

        def outer(x: Int) {mut total} -> Int:
            var local = x * 3

            def inner(y: Int) {imm local} -> Int:
                return y + local

            total += inner(x)
            return total

        return outer(2) + total


def main():
    var ints = Shelf[Int](1)
    ints.items.append(1)
    var words = Shelf[String](2)
    words.items.append("x")
    words.items.append("y")
    print(ints.echoed(5), words.echoed("w"))
    print(ints.fresh(6), words.fresh("f"))
    print(ints.picked(7), words.picked("p"))
    print(ints.counted(), words.counted())
    print(ints.defaulted(3), words.defaulted(4))
    print(ints.copied(3), words.copied(4))
    print(ints.referenced(3), words.referenced(4))
    print(ints.moved(7, 3), words.moved("m", 4))
    print(ints.selfish(3), words.selfish(4))
    print(ints.bumped(3), words.bumped(4))
    print(ints.layered(3), words.layered(4))
    print(ints.deep(3), words.deep(4))
