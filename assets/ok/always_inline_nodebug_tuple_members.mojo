# `Tuple`'s `nodebug` `__len__` and `__getitem_param__` are inlined with the
# tuple's element types bound to its one type pack: over a local, through a
# `mut` parameter, a struct field, a nested tuple, and a tuple typed over the
# caller's own parameters.
def first[A: ImplicitlyCopyable, B: ImplicitlyCopyable](t: Tuple[A, B]) -> A:
    _ = t.__len__()
    return t[0]


def bump(mut t: Tuple[Int, String]):
    t[0] += 10
    t[1] += "!"


struct Holder:
    var pair: Tuple[Int, Bool]

    def __init__(out self):
        self.pair = (7, True)

    def get(self) -> Int:
        return self.pair[0]


def main():
    var t = (1, "a", 2.5)
    print(t.__len__())
    print(t[0], t[1], t[2])
    var p = (3, String("x"))
    bump(p)
    print(p[0], p[1], p.__len__())
    print(first((4, True)))
    var h = Holder()
    print(h.get(), h.pair[1])
    var n = ((1, 2), "z")
    print(n[0][1], n[1], n[0].__len__())
