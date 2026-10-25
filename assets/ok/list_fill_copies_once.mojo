# `List.resize(n, fill)`, `List(length=n, fill=x)`, and `List.copy()` copy
# each element once: the `unsafe_write(copy=v)` that fills the storage reads
# `v` where it lies, as upstream's `init_pointee_copy` reads its `read`
# argument. A temporary fill is copied once and then destroyed.
struct S(Copyable, Movable):
    var n: Int
    var s: String

    def __init__(out self, n: Int):
        self.n = n
        self.s = String("v")

    def __init__(out self, *, copy: Self):
        print("copy", copy.n)
        self.n = copy.n + 10
        self.s = copy.s.copy()

    def __deinit__(deinit self):
        print("del", self.n)


def make() -> S:
    return S(5)


def main():
    var a = S(1)
    var r = List[S]()
    r.resize(2, a)
    print("resize", len(r))
    var l = List[S](length=2, fill=a)
    print("--copy")
    var m = l.copy()
    print("--resize temp")
    var q = List[S]()
    q.resize(1, make())
    print(len(l), len(m), len(q), m[1].n, q[0].n)
    print("end")
