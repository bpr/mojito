# A `for` target unpacking a tuple the iterator yields reads each element
# from the tuple's place, as upstream projects each element from the
# tuple's address: the tuple is not copied first, so no copy constructor
# runs, and each element dies once, after the body.
struct D(Copyable, Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
        print("copy", copy.v)
    def __deinit__(deinit self):
        print("del", self.v)

struct It(Iterator, Copyable):
    comptime Element = Tuple[D, D]
    var i: Int
    def __init__(out self):
        self.i = 0
    def __next__(mut self) raises StopIteration -> Self.Element:
        if self.i >= 1:
            raise StopIteration()
        self.i += 1
        return (D(self.i), D(self.i * 10))
    def __iter__(ref self) -> Self:
        return self.copy()

def main() raises:
    for a, b in It():
        print("body", a.v, b.v)
    print("--- end")
