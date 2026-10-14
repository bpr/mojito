# `comptime for` over a struct that is not itself an iterator: the loop
# iterates whatever its `__iter__()` returns, as upstream desugars the
# iterable to `seq.__iter__()`.
@fieldwise_init
struct Count(Iterator, Copyable):
    comptime Element = Int
    var n: Int

    def __next__(mut self) raises StopIteration -> Int:
        if self.n <= 0:
            raise StopIteration()
        self.n -= 1
        return self.n


@fieldwise_init
struct Bag(Copyable):
    var k: Int

    def __iter__(self) -> Count:
        return Count(self.k)


def main():
    comptime for x in Bag(2):
        print(x)
