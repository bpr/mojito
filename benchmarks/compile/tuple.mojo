# Real Mojo's dependent tuple accessor on a variadic-generic struct:
# `p[k]` with a compile-time-constant k dispatches to
# `def __getitem_param__[i: Int](self) -> Self.Ts[i]`, which unrolls per
# element at specialization, so each subscript has the exact per-index
# element type.
struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable, Sized):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def __getitem_param__[i: Int](self) -> Self.Ts[i]:
        return self.storage[i].copy()

    def __len__(self) -> Int:
        return len(self.storage)


def main():
    var p = Pair[Int, String, Bool](7, "mid", True)
    var n: Int = p[0]
    var s: String = p[1]
    var b: Bool = p[2]
    print(n)
    print(s)
    print(b)
    print(len(p))
    var q = Pair[String, Int]("s", 5)
    print(p[0] + q[1])
