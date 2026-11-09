# A one-field struct declaring `RegisterPassable` unconditionally is held as
# its field even when it owns a destructor or a copy constructor, as the pin
# passes it as `i64`: each lifecycle member still runs on the held value.


@fieldwise_init
struct R(Movable, RegisterPassable):
    var v: Int

    def __deinit__(deinit self):
        print("del R", self.v)


struct C(Copyable, RegisterPassable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
        print("copy C", copy.v)

    def bump(mut self):
        self.v += 1

    def finish(deinit self) -> Int:
        print("finish", self.v)
        return self.v

    def __deinit__(deinit self):
        print("del C", self.v)


@fieldwise_init
struct W(Movable, RegisterPassable):
    var r: R


@fieldwise_init
struct Outer(Copyable):
    var c: C
    var k: Int


def take(var r: R) -> Int:
    return r.v


@no_inline
def pass_through(var c: C) -> C:
    c.bump()
    return c^


def main():
    print(take(R(1)))
    var w = W(R(2))
    print(w.r.v)
    var t = (R(3), 4)
    print(t[1])
    var c = C(5)
    var d = c.copy()
    print(d.v)
    var e = d^
    print(e.v)
    var o = Outer(C(6), 7)
    var p = o.copy()
    print(p.c.v, p.k)
    var moved = p^
    print(moved.c.v)
    var l = List[C]()
    l.append(C(8))
    l.append(C(9))
    print(l[0].v, l[1].v)
    var copied = l.copy()
    print(copied[1].v)
    var m: Optional[C] = C(11)
    print(m.value().v)
    var f = C(12)
    f.bump()
    print(f^.finish())
    var q = pass_through(C(14))
    print(q.v)
