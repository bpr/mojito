# A static member on a `Tuple` type over a pack still open, served by the
# `@staticmethod def __len__()` that `std/builtin/tuple.mojo` declares: over
# the def's own pack, its reversal, a `TypeList._concat`, and a struct's
# `Self.Ts`, in a body and in a `comptime` initializer.


struct Holder[*Ts: Movable]:
    var n: Int

    def __init__(out self):
        self.n = 0

    def join[*OtherTs: Movable](self, other: Tuple[*OtherTs]) -> Int:
        return Tuple[
            *TypeList._concat[Self.Ts.values, OtherTs.values]()
        ].__len__()

    def own_len(self) -> Int:
        return Tuple[*Self.Ts].__len__()

    def rev_len(self) -> Int:
        return Tuple[*Self.Ts.reverse()].__len__()


def plain_len[*Ts: Movable](t: Tuple[*Ts]) -> Int:
    return Tuple[*Ts].__len__()


def rev_len[*Ts: Movable](t: Tuple[*Ts]) -> Int:
    return Tuple[*Ts.reverse()].__len__()


def cat_len[*Ts: Movable](t: Tuple[*Ts]) -> Int:
    return Tuple[*TypeList._concat[Ts.values, Ts.reverse().values]()].__len__()


def comptime_len[*Ts: Movable](t: Tuple[*Ts]) -> Int:
    comptime n = Tuple[*Ts].__len__()
    return n


def main():
    print(plain_len((1, "a", 2.5)))
    print(rev_len((1, "a", 2.5)))
    print(cat_len((1, "a", 2.0)))
    print(comptime_len((1, "a")))
    var h = Holder[Int, Bool]()
    print(h.join(Tuple(String("a"), 2.0)))
    print(h.own_len())
    print(h.rev_len())
