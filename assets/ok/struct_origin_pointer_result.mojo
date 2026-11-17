# A member's `Pointer[T, Self.o]` result names the receiver's origin
# argument, and a free function's result names the binder a struct
# argument's tail bound, so each converts to `Pointer[T, origin_of(x)]`.


struct Noisy(Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


struct View[mut: Bool, //, o: Origin[mut=mut]](TrivialRegisterPassable):
    var p: Pointer[Noisy, Self.o]

    def __init__(out self, p: Pointer[Noisy, Self.o]):
        self.p = p

    def get(self) -> Pointer[Noisy, Self.o]:
        return self.p

    def __getitem__(self, i: Int) -> Pointer[Noisy, Self.o]:
        return self.p


struct Boxed[mut: Bool, //, o: Origin[mut=mut]](Movable):
    var p: Pointer[Noisy, Self.o]

    def __init__(out self, p: Pointer[Noisy, Self.o]):
        self.p = p

    def get(ref self) -> Pointer[Noisy, Self.o]:
        return self.p


def through[mut: Bool, //, o: Origin[mut=mut]](v: View[o]) -> Pointer[Noisy, o]:
    return v.get()


def second[
    o: MutOrigin, mut: Bool, //, q: Origin[mut=mut]
](a: View[o], v: View[q]) -> Pointer[Noisy, q]:
    return v.get()


def main():
    var x = Noisy(7)
    var v = View(Pointer(to=x))
    var p: Pointer[Noisy, origin_of(x)] = v.get()
    print(p[].v)
    var q: Pointer[Noisy, origin_of(x)] = v[0]
    print(q[].v)
    var b = Boxed(Pointer(to=x))
    var r: Pointer[Noisy, origin_of(x)] = b.get()
    print(r[].v)
    var t: Pointer[Noisy, origin_of(x)] = through(v)
    print(t[].v)
    var y = Noisy(8)
    var w = View(Pointer(to=y))
    print(second(w, v)[].v)
    var s = v.get()
    s[].v = 9
    print(x.v)
