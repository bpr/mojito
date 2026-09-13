# Borrowed iteration over a temporary whose iterator borrows nothing: the source
# is destroyed exactly once, as soon as `__iter__` returns, before the first
# element. `Numbers(3)` is the only owner of its storage and `NumbersIter`
# carries no origin, so nothing keeps the source alive through the loop — the
# ASAP rule current Mojo applies. The source still keeps its own slot, apart
# from the iterator object, so normalization cannot overwrite (leak) it.
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
    for x in Numbers(3):
        print("x", x)
    print("after")
