# Type-level pack algebra over packs still open: a reversal and a
# concatenation, measured and indexed.


struct Holder[*Ts: Movable]:
    var n: Int

    def __init__(out self):
        self.n = 0

    def join[*OtherTs: Movable](self, other: Tuple[*OtherTs]) -> Int:
        comptime L = TypeList._concat[Self.Ts.values, OtherTs.values]()
        return L.length

    def flipped_first(self) -> Int:
        comptime if Self.Ts.reverse()[0] == Bool:
            return 1
        return 0

    def flipped_len(self) -> Int:
        return Self.Ts.reverse().length


def doubled_len[*Ts: Movable](t: Tuple[*Ts]) -> Int:
    return TypeList._concat[Ts.values, Ts.reverse().values]().length


def last_is_string[*Ts: Movable](t: Tuple[*Ts]) -> Bool:
    comptime if Ts.reverse()[0] == String:
        return True
    return False


def main():
    var h = Holder[Int, Bool]()
    print(h.join(Tuple(String("a"), 2.0)))
    print(h.flipped_first())
    print(h.flipped_len())
    print(doubled_len((1, "a", 2.0)))
    print(last_is_string((1, String("s"))), last_is_string((String("s"), 1)))
