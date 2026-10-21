# Starred sequence targets and patterns in their evaluation order over a
# user `Sized` type. A target checks a minimum length, reads `__len__` once
# more for the suffix start, then stores the prefix, the slice (`*_`
# included), and the suffix in order. A pattern reads `__len__` afresh for
# each from-end element and slice end, and `*_` slices nothing.
from std.builtin.builtin_slice import ContiguousSlice

struct Seq(Sized, ImplicitlyCopyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __len__(self) -> Int:
        print("len")
        return self.n

    def __getitem__(self, i: Int) -> Int:
        print("get", i)
        return 10 + i

    def __getitem__(self, s: ContiguousSlice) -> Seq:
        print("slice", s.start.or_else(-1), s.end.or_else(-1))
        return Seq(0)


def main() raises:
    var t = Seq(5)
    print("-- target [a, *m, b, c]")
    var [a, *m, b, c] = t
    print(a, b, c)
    print("-- target [a, *_, b]")
    var [p, *_, q] = t
    print(p, q)
    print("-- target [*r]")
    var [*r] = t
    print("-- target [x, *r2]")
    var [x, *r2] = t
    print(x)
    print("-- pattern [a, *m, b, c]")
    match t:
        case [u, *mm, v, w]:
            print(u, v, w)
        case _:
            print("other")
    print("-- pattern [*m, 14]")
    match t:
        case [*mm, 14]:
            print("ends")
        case _:
            print("other")
    print("-- pattern [*_, b, c]")
    match t:
        case [*_, v, w]:
            print(v, w)
        case _:
            print("other")
    print("-- pattern too short")
    match t:
        case [_, _, _, *mm, _, _, _]:
            print("long")
        case _:
            print("short")
