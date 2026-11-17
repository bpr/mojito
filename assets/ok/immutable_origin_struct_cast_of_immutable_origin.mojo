# The `ImmOrigin(...)` cast of an origin that is already immutable is that
# origin: a struct type converts between the two slots in either direction
# over a `read` owner, over a binder declared `ImmOrigin`, and where a
# generic result's `ImmOrigin(o)` binds `o` to such an origin, as at the pin.
struct Noisy(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


struct View[mut: Bool, //, o: Origin[mut=mut]](Movable):
    var p: Pointer[Noisy, Self.o]

    def __init__(out self, p: Pointer[Noisy, Self.o]):
        self.p = p


def mk[o: Origin](p: Pointer[Noisy, o]) -> View[ImmOrigin(o)]:
    return View[ImmOrigin(o)](p)


def read_owner(s: Noisy):
    var v = View[ImmOrigin(origin_of(s))](Pointer(to=s))
    var w: View[origin_of(s)] = v^
    var u: View[ImmOrigin(origin_of(s))] = w^
    var r: View[origin_of(s)] = mk(Pointer(to=s))
    print(u.p[].v, r.p[].v)


def imm_binder[o: ImmOrigin](v: View[o]):
    var w: View[ImmOrigin(o)] = View(v.p)
    var u: View[o] = mk(w.p)
    print(u.p[].v)


def main():
    var x = Noisy(1)
    read_owner(x)
    imm_binder(View[ImmOrigin(origin_of(x))](Pointer(to=x)))
