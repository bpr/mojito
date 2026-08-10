# Borrowed iteration over a temporary keeps the source alive in its own slot and
# destroys it exactly once, after the loop. `Numbers(3)` is the only owner of its
# storage; its `__iter__(self)` returns a borrowing iterator. Before the source
# and iterator were given distinct slots, normalization overwrote the source in
# place, so its `__deinit__` never ran (a leak). The expected output pins the drop
# after the final element, before execution continues.
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

    def __deinit__(deinit self):
        print("drop numbers", self.stop)

    def __iter__(self) -> NumbersIter:
        return NumbersIter(0, self.stop)

def main():
    for x in Numbers(3):
        print("x", x)
    print("after")
