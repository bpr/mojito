# `comptime for` over an iterator that is a generic struct instance: each
# copy's variable is a parameter value (`show[x]()`), its iterator the frozen
# instance bound at the declared closed type (`Up[2]`, `_ZeroStartingRange`),
# as upstream's `paramfor_*` stubs step it.
@fieldwise_init
struct Up[step: Int](Iterator, Copyable):
    comptime Element = Int
    var i: Int
    var n: Int

    def __iter__(ref self) -> Self:
        return self.copy()

    def __next__(mut self) raises StopIteration -> Int:
        if self.i >= self.n:
            raise StopIteration()
        var v = self.i
        self.i += Self.step
        return v


def show[n: Int]():
    print(n, "/ n =", n)


def main():
    comptime for x in Up[2](0, 5):
        show[x]()
    comptime for x in range(3).__iter__():
        show[x]()
    comptime for x in range(1, 7, 2).__iter__():
        print(x * 10)
    comptime for x in range(5, 1, -2).__iter__():
        show[x]()
