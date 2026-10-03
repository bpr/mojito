# A collection display default takes its parameter's type as context: `[1, 2]`
# is a fresh `List[Int]` on each call that leaves it out, not an `Array`.
from std.collections import Set


struct Bag:
    var n: Int

    def __init__(out self):
        self.n = 0

    def fill(mut self, var xs: List[Int] = [4, 5, 6]):
        for x in xs:
            self.n += x


def grow(var xs: List[Int] = [1, 2]) -> Int:
    xs.append(3)
    return len(xs)


def floats(xs: List[Float64] = [1, 2.5]) -> Float64:
    return xs[0] + xs[1]


def keys(s: Set[Int] = {1, 2, 3}) -> Int:
    return len(s)


def table(d: Dict[String, Int] = {"a": 1}) raises -> Int:
    return d["a"]


def empty(var xs: List[String] = []) -> Int:
    xs.append("x")
    return len(xs)


def main() raises:
    print(grow())
    print(grow())
    print(grow([5]))
    var b = Bag()
    b.fill()
    b.fill([1])
    print(b.n)
    print(floats())
    print(keys())
    print(table())
    print(empty())
    print(empty())
