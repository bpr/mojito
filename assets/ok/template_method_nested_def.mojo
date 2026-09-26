# A per-instantiation method clone inherits its checked template's facts when
# the body declares and calls a nested `def`
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `NESTED_DEFS`). The nested signature is closed scalars, which no instance
# changes, and each capture names a local or a parameter of the method, which
# an instance maps to its own binding, whatever the captured type.


struct Shelf[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]
    var bias: Int

    def __init__(out self, bias: Int):
        self.items = List[Self.T]()
        self.bias = bias

    def scaled(self, k: Int) -> Int:
        var base = len(self.items)

        def times(x: Int) {imm base} -> Int:
            return base * x

        return times(k) + times(1)

    def counted(self, n: Int) -> Int:
        var total = 0

        def add(x: Int) {mut total}:
            total += x

        for i in range(n):
            add(i)
        add(self.bias)
        return total

    def snapshot(self, k: Int) -> Int:
        var seed = self.bias

        def later(x: Int) {var seed} -> Int:
            var doubled = seed * 2
            if x > 0:
                return doubled + x
            return doubled

        seed = 100
        return later(k) + seed

    def mixed(self, k: Int, m: Int) -> Int:
        def mix(x: Int) {imm k, imm m} -> Int:
            return x * k + m

        return mix(2)

    def held(self, item: Self.T, k: Int) -> Int:
        def probe(x: Int) {imm item} -> Int:
            return x + 1

        return probe(k)

    def plain(self, k: Int) -> Int:
        def twice(x: Int) -> Int:
            return x * 2

        return twice(k)


def main():
    var ints = Shelf[Int](1)
    ints.items.append(1)
    var words = Shelf[String](2)
    words.items.append("x")
    words.items.append("y")
    print(ints.scaled(3), words.scaled(4))
    print(ints.counted(4), words.counted(5))
    print(ints.snapshot(3), words.snapshot(0))
    print(ints.mixed(3, 4), words.mixed(5, 6))
    print(ints.held(1, 3), words.held("a", 4))
    print(ints.plain(3), words.plain(4))
