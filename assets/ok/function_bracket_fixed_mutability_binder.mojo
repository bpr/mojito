# A call's bracket fixes a shared `mut=` binder before the arguments bind
# the origins it governs: `t=ImmOrigin(...)` or `t=ImmUntrackedOrigin`
# fixes `m` to `False`, which a mutable or an immutable argument origin
# converts to, and `t=MutUntrackedOrigin` or `m=True` fixes it to `True`,
# which a mutable argument satisfies; a construction's `V[m=...]` bracket
# does the same (pinned Mojo: prints 4, 3, 4, 5, 3, 4, then 4).
struct V[m: Bool, //, o: Origin[mut=m]]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p


def f[m: Bool, //, o: Origin[mut=m], t: Origin[mut=m]](p: Pointer[Int, o]) -> Int:
    return p[]


def h[m: Bool, //, o: Origin[mut=m]](p: Pointer[Int, o]) -> Int:
    return p[] + 1


def g(q: Pointer[Int, ImmUntrackedOrigin]) -> Int:
    return f[t=ImmUntrackedOrigin](q)


def k(q: Pointer[Int, ImmUntrackedOrigin]) -> Int:
    var v = V[m=False](q)
    return v.p[]


def main():
    var y = 4
    var x = 3
    print(f[t=ImmOrigin(origin_of(x))](Pointer(to=y)))
    print(g(Pointer(to=x).as_imm().unsafe_origin_cast[ImmUntrackedOrigin]()))
    print(f[t=MutUntrackedOrigin](Pointer(to=y)))
    print(h[m=True](Pointer(to=y)))
    print(k(Pointer(to=x).as_imm().unsafe_origin_cast[ImmUntrackedOrigin]()))
    var w = V[m=True](Pointer(to=y))
    print(w.p[])
    var u = V[m=False](Pointer(to=y))
    print(u.p[])
    _ = x
