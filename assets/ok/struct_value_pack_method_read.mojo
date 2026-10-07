# A struct's own value pack read in its methods: its length, an element at a
# compile-time index, a `comptime for` over its elements or its indices, and
# an element materialized under a runtime comparison.


struct Pack[*values: Int](Sized):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        return len(Self.values)

    def first(self) -> Int:
        return Self.values[0]

    def total(self) -> Int:
        var sum = 0
        comptime for value in Self.values:
            sum += value
        return sum

    def weighted(self) -> Int:
        var sum = 0
        comptime for i in range(len(Self.values)):
            sum += (i + 1) * Self.values[i]
        return sum

    def get(self, index: Int) -> Int:
        var found = -1
        comptime for i in range(len(Self.values)):
            if i == index:
                found = materialize[Self.values[i]]()
        return found


struct Flags[*flags: Bool]:
    def __init__(out self):
        pass

    def count(self) -> Int:
        var n = 0
        comptime for flag in Self.flags:
            if flag:
                n += 1
        return n


def main():
    var p = Pack[3, 5, 7]()
    print(len(p))
    print(p.first())
    print(p.total())
    print(p.weighted())
    print(p.get(1))
    print(p.get(9))
    print(Flags[True, False, True]().count())
