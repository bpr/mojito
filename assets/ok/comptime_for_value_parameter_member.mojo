# A struct value's declared parameter (`a.size`) is its type's parameter,
# so a `comptime for` may range over it whatever the value holds.
from std.utils import StaticTuple


struct Row[n: Int](ImplicitlyCopyable):
    var first: Int

    def __init__(out self, first: Int):
        self.first = first


def count(a: StaticTuple[Bool, _]) -> Int:
    var c = 0
    comptime for i in range(a.size):
        if a[i]:
            c += 1
    return c


def total(r: Row) -> Int:
    var sum = r.first
    comptime for i in range(r.n):
        sum += i
    return sum


def main():
    print(count(StaticTuple[Bool, 3](True, False, True)))
    print(total(Row[4](10)))
