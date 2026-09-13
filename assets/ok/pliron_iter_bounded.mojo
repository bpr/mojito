# A bounded user iterator: the raising `__next__(mut self)` advances the
# iterator variable's own storage in place and raises `StopIteration` past its
# bound, and `continue`/`break` leave the iterator to the loop's cleanup drops.
@fieldwise_init
struct CountIter:
    var cur: Int
    var stop: Int

    def __next__(mut self) raises StopIteration -> Int:
        if self.cur >= self.stop:
            raise StopIteration()
        var value: Int = self.cur
        self.cur = self.cur + 1
        return value

@fieldwise_init
struct Counts:
    var n: Int

    def __iter__(self) -> CountIter:
        return CountIter(0, self.n)

def main():
    var total: Int = 0
    for x in Counts(8):
        if x == 2:
            continue
        if x == 6:
            break
        total = total + x
    print(total)
