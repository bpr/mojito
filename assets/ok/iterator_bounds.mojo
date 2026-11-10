# `Iterator` refines `Deinitable` and `Movable` and declares a defaulted
# `bounds()` (`(0, None)`), which the bundled iterators override with their
# exact remaining length, as upstream's do; `List(iterable)` sizes itself by
# the iterable's lower bound.
struct Count(Iterator):
    comptime Element = Int
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __next__(mut self) raises StopIteration -> Int:
        if self.n == 0:
            raise StopIteration()
        self.n -= 1
        return self.n


struct Exact(Iterator):
    comptime Element = Int
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __next__(mut self) raises StopIteration -> Int:
        if self.n == 0:
            raise StopIteration()
        self.n -= 1
        return self.n

    def bounds(self) -> Tuple[Int, Optional[Int]]:
        return (self.n, {self.n})


def lower[I: Iterator](it: I) -> Int:
    var lo, hi = it.bounds()
    if hi:
        return lo + hi.value() * 100
    return lo - 1

def main() raises:
    var xs: List[Int] = [1, 2, 3]
    var it = xs.__iter__()
    print(lower(it))
    _ = next(it)
    print(it.bounds()[0])
    var arr = [4, 5, 6, 7]
    print(lower(arr.__iter__()))
    print(lower(range(5).__iter__()))
    print(lower(range(2, 9)))
    print(lower(range(0, 10, 3)))
    print(lower(reversed(xs)))
    var s = "abc"
    print(lower(s.codepoints()))
    var d = {1: "a", 2: "b"}
    print(lower(d.keys()))
    print(lower(d.values()))
    print(lower(d.items()))
    var st = {1, 2, 3, 4}
    print(lower(st.__iter__()))
    print(lower(Span(xs).__iter__()))
    var o = Optional(7)
    print(lower(o.__iter__()))
    print(o.bounds()[0])
    var big = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17]
    var copy = List(big)
    print(len(copy), copy.capacity())
    print(lower(String("hey").as_bytes().__iter__()))
    print(lower(xs^.__iter__()))
    print(lower(arr^.__iter__()))
    print(lower(Count(3)))
    var e = Exact(4)
    print(lower(e))
    var moved = e^
    print(moved.bounds()[0])
