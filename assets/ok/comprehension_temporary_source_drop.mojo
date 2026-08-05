# A comprehension over a borrowed temporary keeps the source alive in its own
# slot and destroys it exactly once, after the comprehension. `Numbers(3)` is
# the only owner of its storage; its `__iter__(self)` returns a borrowing
# iterator. Before comprehensions shared the statement loop's retained-source/
# iterator-object slot split, normalization overwrote the source in place, so
# its `__del__` never ran (a leak). The expected output pins the drop after the
# comprehension, before execution continues.
@fieldwise_init
struct NumbersIter:
    var cur: Int
    var stop: Int

    def __len__(self) -> Int:
        return self.stop - self.cur

    def __next__(mut self) -> Int:
        var v = self.cur
        self.cur = self.cur + 1
        return v

struct Numbers(Movable):
    var stop: Int

    def __init__(out self, stop: Int):
        self.stop = stop

    def __init__(out self, *, deinit move: Self):
        self.stop = move.stop

    def __del__(deinit self):
        print("drop numbers", self.stop)

    def __iter__(self) -> NumbersIter:
        return NumbersIter(0, self.stop)

def main():
    var values = [x for x in Numbers(3)]
    for v in values:
        print("x", v)
    print("after")
