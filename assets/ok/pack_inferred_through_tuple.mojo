# A callable's own type pack is inferred from a `Tuple[*Ts]` parameter: the
# argument's element list binds the pack whole.


def count[*Ts: Movable](other: Tuple[*Ts]) -> Int:
    return Ts.length


def both[*Ts: Movable](other: Tuple[*Ts], *rest: *Ts) -> Int:
    return Ts.length + len(rest)


def first_of[*Ts: Writable & Movable](other: Tuple[*Ts]) -> String:
    return String(other[0])


def inner[*Ts: Movable](x: Tuple[*Ts]) -> Int:
    return Ts.length


# The caller's own pack forwards through the parameter whole.
def outer[*Us: Movable](t: Tuple[*Us]) -> Int:
    return inner(t)


struct Holder[*Ts: Movable]:
    var n: Int

    def __init__(out self):
        self.n = 0

    def count[*OtherTs: Movable](self, other: Tuple[*OtherTs]) -> Int:
        return Self.Ts.length + OtherTs.length

    def owned[*OtherTs: Movable & Deinitable](self, var other: Tuple[*OtherTs]) -> Int:
        return OtherTs.length


def main():
    print(count((1, "a", 2.0)))
    print(count[Int, Bool](Tuple(1, True)))
    print(count(Tuple(7)))
    print(count(Tuple()))
    var named = Tuple(String("x"), 3)
    print(count(named), named[0])
    print(both((1, True), 2, False))
    print(first_of((4, True)))
    print(outer((1, True, 2.5)), inner(Tuple(4)))
    var h = Holder[Int, Bool]()
    print(h.count(Tuple(1, "a", 2.0)))
    print(h.count[String](Tuple(String("s"))))
    print(h.owned((1, 2)))
