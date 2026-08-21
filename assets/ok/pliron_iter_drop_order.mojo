# Drop order around borrowed raising iteration: the temporary source lives
# in its own split slot for the whole loop and destroys exactly once, after
# the final element and before execution continues; a `break` leaves the
# iterator to the loop's cleanup drops without touching the source's single
# destruction.
from std.iterable import StopIteration

@fieldwise_init
struct TraceIter:
    var cur: Int
    var stop: Int

    def __next__(mut self) raises StopIteration -> Int:
        if self.cur >= self.stop:
            raise StopIteration()
        var value: Int = self.cur
        self.cur = self.cur + 1
        return value

struct Tracked(Movable):
    var tag: Int
    var stop: Int

    def __init__(out self, tag: Int, stop: Int):
        self.tag = tag
        self.stop = stop

    def __iter__(ref self) -> TraceIter:
        return TraceIter(0, self.stop)

    def __deinit__(deinit self):
        print("dropped", self.tag)

def main():
    for x in Tracked(1, 3):
        print("saw", x)
    print("between")
    for y in Tracked(2, 9):
        if y == 1:
            break
        print("kept", y)
    print("done")
