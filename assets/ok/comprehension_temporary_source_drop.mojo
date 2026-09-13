# A comprehension over a temporary whose iterator borrows nothing destroys the
# source exactly once, as soon as `__iter__` returns. `Numbers(3)` is the only
# owner of its storage and `NumbersIter` carries no origin, so the comprehension
# shares the statement loop's ASAP rule; the source still keeps its own slot,
# apart from the iterator object, so normalization cannot overwrite (leak) it.
@fieldwise_init
struct NumbersIter:
    var cur: Int
    var stop: Int

    def __next__(mut self) raises StopIteration -> Int:
        if self.cur >= self.stop:
            raise StopIteration()
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
    var values = [x for x in Numbers(3)]
    for v in values:
        print("x", v)
    print("after")
