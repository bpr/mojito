# A struct built over arithmetic on a compile-time parameter
# (`Counter[1 + Self.length](...)` in a method, `Counter[n + 1](...)` in a
# generic `def`) evaluates its bracket argument in the erased body, which
# every backend verifies with checked types.
struct Counter[length: Int](Copyable, Movable):
    var i: Int

    def __init__(out self, i: Int):
        self.i = i

    def size(self) -> Int:
        return Self.length

    def wider(self) -> Counter[1 + Self.length]:
        var c: Counter[1 + Self.length] = Counter[1 + Self.length](self.i + 1)
        return c^

    def scaled(self) -> Int:
        return Counter[2 * Self.length - 1](self.i).size()


def grow[n: Int](i: Int) -> Int:
    return Counter[n + 1](i).size()


def main():
    var c = Counter[4](1)
    print(c.wider().size(), c.wider().i, c.scaled(), grow[4](c.i))
    print(Counter[4](1).wider().wider().size())
