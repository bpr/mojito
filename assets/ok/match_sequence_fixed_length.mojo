# Fixed-length sequence patterns: one length test (`list_length_matches`)
# gates every element read; elements read in pattern order, once per case
# or `|` alternative, `_` reading nothing; owned getter results bind by
# move and die after their last use. Over `List`, user `Sized` types, a
# generic `Sized` subject, a temporary, and inside a tuple pattern.
struct Trio(Sized, ImplicitlyCopyable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __len__(self) -> Int:
        print("len")
        return 3

    def __getitem__(self, i: Int) -> Int:
        print("get", i)
        return self.base + i


struct Tracked(Copyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n
        print("copy", self.n)

    def __deinit__(deinit self):
        print("del", self.n)

    def __eq__(self, other: Int) -> Bool:
        return self.n == other


struct Maker(Sized, Copyable):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        return 2

    def __getitem__(self, i: Int) -> Tracked:
        print("get", i)
        return Tracked(i)


trait Seq(Sized):
    def __getitem__(self, i: Int) -> Int:
        ...


struct Two(Seq, Copyable):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        return 2

    def __getitem__(self, i: Int) -> Int:
        return i + 7


def pair_sum[T: Seq](x: T) -> Int:
    match x:
        case [a, b]:
            return a + b
        case _:
            return -1


def make() -> List[Int]:
    print("make")
    return [4, 5, 6]


def main():
    var t = Trio(10)
    match t:
        case [a, b]:
            print("two", a, b)
        case [10, _, c]:
            print("three", c)
        case _:
            print("other")
    match t:
        case [a, 99, c]:
            print("never", a, c)
        case [var a, 1, _] | [var a, 11, _]:
            print("alt", a)
        case _:
            print("other")
    match t:
        case [10 | 11 | 12, _, var c] if c > 100:
            print("never", c)
        case [x, _, _]:
            print("first", x)
        case _:
            print("other")
    var xs: List[Int] = [1, 2, 3]
    match xs:
        case []:
            print("empty")
        case [x]:
            print("one", x)
        case [_, ref m, _]:
            m = 20
            print("three", m)
        case _:
            print("other")
    print(xs[1], xs[2])
    match xs:
        case [1, _, var z]:
            z += 30
            print("copied", z)
        case _:
            print("other")
    print(xs[2])
    var empty = List[Int]()
    match empty:
        case [x]:
            print("one", x)
        case []:
            print("empty")
        case _:
            print("other")
    match (xs.copy(), 1):
        case ([a, _, _], 1):
            print("in tuple", a)
        case _:
            print("other")
    match (t, 1):
        case ([_, _, 12], 1):
            print("trio in tuple")
        case _:
            print("other")
    print(pair_sum(Two()))
    match make():
        case [_, _, c]:
            print("temporary", c)
        case _:
            print("other")
    var mk = Maker()
    match mk:
        case [a, 5]:
            print("never", a.n)
        case [b, _]:
            print("bare", b.n)
        case _:
            print("other")
    match mk:
        case [var c, 1]:
            print("moved", c.n)
        case _:
            print("other")
    if case [_, b, _] = xs:
        print("if case", b)
