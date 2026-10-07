# A value pack still a parameter spread whole into a bracket: a struct's
# arguments (`Pack[*vs]`, `Pack[*Self.qs]`), a generic `def`'s
# (`total[*vs]()`), and a method's (`Sink().count[*vs]()`).


struct Pack[*values: Int](Sized):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        return len(Self.values)

    def last(self) -> Int:
        return Self.values[len(Self.values) - 1]


struct Forward[*qs: Int]:
    def __init__(out self):
        pass

    def size(self) -> Int:
        return len(Pack[*Self.qs]())


struct Sink:
    def __init__(out self):
        pass

    def count[*xs: Int](self) -> Int:
        return len(xs)


def total[*xs: Int]() -> Int:
    var sum = 0
    comptime for x in xs:
        sum += x
    return sum


def wrap[*vs: Int]() -> Int:
    return len(Pack[*vs]()) * 10 + Pack[*vs]().last()


def forward[*vs: Int]() -> Int:
    return total[*vs]()


def forward_method[*vs: Int]() -> Int:
    return Sink().count[*vs]()


def main():
    print(wrap[1, 2]())
    print(forward[4, 5, 6]())
    print(forward_method[4, 5, 6]())
    print(Forward[1, 2, 3, 4]().size())
