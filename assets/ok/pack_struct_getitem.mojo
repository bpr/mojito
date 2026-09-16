# A variadic struct's dependent accessor `def __getitem__[i: Int](self) ->
# Self.Ts[i]` unrolls per element at specialization, so an explicit
# `p.__getitem__[k]()` with a compile-time `k` has the exact per-index
# element type — the pinned Mojo's spelling, which reads the brackets of
# `p[k]` as call arguments. The element is `Copyable`, not implicitly
# copyable, so the accessor copies it out explicitly; `len(p)` needs the
# declared `Sized` conformance.
struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable, Sized):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def __getitem__[i: Int](self) -> Self.Ts[i]:
        return self.storage[i].copy()

    def __len__(self) -> Int:
        return len(self.storage)


def main():
    var p = Pair[Int, String, Bool](7, "mid", True)
    var n: Int = p.__getitem__[0]()
    var s: String = p.__getitem__[1]()
    var b: Bool = p.__getitem__[2]()
    print(n)
    print(s)
    print(b)
    print(len(p))
    var q = Pair[String, Int]("s", 5)
    print(p.__getitem__[0]() + q.__getitem__[1]())
