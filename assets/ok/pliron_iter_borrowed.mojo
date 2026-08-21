# Borrowed raising iteration through both source shapes: a named source
# binds the loop's source slot to a reference handle — the step-0
# `__iter__(ref self)` receiver aliases the caller's storage — while a
# temporary source stays live in its own split slot until after the loop.
from std.iterable import StopIteration

@fieldwise_init
struct WindowIter:
    var cur: Int
    var stop: Int

    def __next__(mut self) raises StopIteration -> Int:
        if self.cur >= self.stop:
            raise StopIteration()
        var value: Int = self.cur
        self.cur = self.cur + 1
        return value

@fieldwise_init
struct Window:
    var lo: Int
    var hi: Int

    def __iter__(ref self) -> WindowIter:
        return WindowIter(self.lo, self.hi)

def main():
    var named = Window(1, 4)
    var total: Int = 0
    for x in named:
        total = total + x
    for y in Window(10, 13):
        total = total + y
    print(total)
