# A reference result binds a `ref`, `mut`, or `read` parameter to the
# referent it names, as the pin does: a collection element of a type that
# is only `Copyable` passes, a write lands in the element, and no copy runs.


struct M(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        print("copy", copy.v)
        self.v = copy.v + 100


def pick(ref m: M) -> ref[m] M:
    return m


def peek(ref x: M) -> Int:
    return x.v


def peek_any[T: Copyable](ref x: T) -> Int:
    return 1


def bump(mut m: M):
    m.v += 1


def show(m: M):
    print("show", m.v)


def main() raises:
    var l = List[M]()
    l.append(M(1))
    print(peek(l[0]), peek_any(l[0]))
    bump(l[0])
    bump(pick(l[0]))
    var d = Dict[String, M]()
    d["a"] = M(10)
    bump(d["a"])
    var a = M(20)
    show(pick(a))
    show(pick(l[0]))
    print(l[0].v, d["a"].v, a.v)
