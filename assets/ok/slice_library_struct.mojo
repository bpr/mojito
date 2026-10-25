# `Slice` and `slice(...)` are upstream's library code, prelude names: a
# literal bound to a `Slice` parameter calls its `__slice_literal__`
# constructor, so `None` and `Optional[Int]` operands pass; `==`, the
# printed and repr forms, `indices`, and the bounds are the library's
# bodies and fields; and a `Slice` converts to `StridedSlice` implicitly.


struct C:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __getitem__(self, s: Slice) -> Int:
        var t = s.indices(self.n)
        return t[0] * 100 + t[1] * 10 + t[2]


def main():
    var c = C(5)
    var o: Optional[Int] = 2
    print(c[1:3])
    print(c[1:3:None])
    print(c[None:3])
    print(c[o:])
    print(c[::-1])
    var a = Slice(1, 3)
    var b = Slice(1, 3, None)
    print(a == b, a != b)
    print(a)
    print(repr(a))
    print(String(a))
    print(a.start.value(), a.end.value(), Bool(a.step))
    var s = slice(4)
    print(s)
    var t = s.indices(10)
    print(t[0], t[1], t[2])
    var none: Optional[Int] = None
    print(Slice(none, 3, -1))
    print(Slice(None, None, -1).indices(4))
    var xs: List[Int] = [0, 1, 2, 3, 4, 5]
    print(xs[Slice(1, 5, 2)])
