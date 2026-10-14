# `comptime for` over a user `Iterator & Copyable`: the loop iterates the
# iterable's `__iter__()`, stepped at compile time by the bundled
# `paramfor_*` stubs, as upstream's elaborator steps it. Each copy's variable
# is the element `paramfor_next_value` gives for its iterator.
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


@fieldwise_init
struct Repeat[T: ImplicitlyCopyable & Writable & Deinitable](Iterator, Copyable):
    comptime Element = Self.T
    var value: Self.T
    var n: Int

    def __next__(mut self) raises StopIteration -> Self.T:
        if self.n <= 0:
            raise StopIteration()
        self.n -= 1
        return self.value

    def __iter__(self) -> Self:
        return self.copy()


def countdown[n: Int]():
    comptime for x in Count(n):
        comptime if x == 1:
            continue
        print("countdown", x)


def nested[k: Int]():
    comptime for x in Count(k):
        comptime for y in Count(x):
            print("pair", x, y)


def main():
    comptime for x in Count(3):
        print(x)
    countdown[4]()
    comptime for x in Count(5):
        if x == 2:
            break
        print("b", x)
    nested[3]()
    comptime counter = Count(2)
    comptime for z in counter:
        print("z", z)
    comptime for flag in Repeat[Bool](True, 2):
        print(flag)
