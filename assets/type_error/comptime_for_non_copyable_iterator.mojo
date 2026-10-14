# expect: type 'Once' for parameter 'IteratorType' does not conform to trait 'Copyable'
# The compile-time stepping stubs copy the iterator, so a `comptime for`
# iterator must be `Copyable`, as upstream's `paramfor_has_next` requires.
struct Once(Iterator, Movable):
    comptime Element = Int
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __next__(mut self) raises StopIteration -> Int:
        if self.n <= 0:
            raise StopIteration()
        self.n -= 1
        return self.n

    def __iter__(var self) -> Self:
        return self^


def main():
    comptime for x in Once(2):
        print(x)
