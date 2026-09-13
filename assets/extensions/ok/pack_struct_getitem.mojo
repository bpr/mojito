# Mojito's subscript sugar `p[k]` selects a *parametric* `__getitem__[i: Int]`,
# binding the bracket as the compile-time parameter, and copies the dependent
# pack element out. Upstream reads the brackets as call arguments (the
# accessor has to be spelled `p.__getitem__[k]()`) and refuses the copy
# without a `rebind`, which Mojito does not implement.
# Real Mojo's dependent tuple accessor on a variadic-generic struct:
# `def __getitem__[i: Int](self) -> Ts[i]` unrolls per element at
# specialization, so `p[k]` (compile-time-constant k) has the exact
# per-index element type.
struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def __getitem__[i: Int](self) -> Self.Ts[i]:
        return self.storage[i]

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
