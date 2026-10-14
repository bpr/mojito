# expect: cannot use a dynamic value in 'for' iterator expression
# A `comptime for` iterable is a compile-time value: a runtime variable
# holding an iterator is not one.
@fieldwise_init
struct Count(Iterator, Copyable):
    comptime Element = Int
    var n: Int

    def __next__(mut self) raises StopIteration -> Int:
        if self.n <= 0:
            raise StopIteration()
        self.n -= 1
        return self.n

    def __iter__(self) -> Self:
        return self.copy()


def main():
    var counter = Count(2)
    comptime for x in counter:
        print(x)
