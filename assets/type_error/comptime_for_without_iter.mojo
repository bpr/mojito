# expect: 'Count' does not implement the '__iter__' method
# A `comptime for` iterates its iterable's `__iter__()`, as upstream: an
# iterator with no `__iter__` is not iterable.
@fieldwise_init
struct Count(Iterator, Copyable):
    comptime Element = Int
    var n: Int

    def __next__(mut self) raises StopIteration -> Int:
        if self.n <= 0:
            raise StopIteration()
        self.n -= 1
        return self.n


def main():
    comptime for x in Count(3):
        print(x)
