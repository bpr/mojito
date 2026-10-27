# A value of a struct declaring `TrivialRegisterPassable` with one field is
# held as that field, as upstream lowers a one-element register-passable
# struct to its element: construction, field reads and writes through
# `mut self`, nested fields, a `ref` binding, heap slots, `List`, `Optional`,
# display, operators, and a compile-time value all behave as for any
# struct, and a struct holding a pointer stays one.
from std.memory.alloc import unsafe_alloc


@fieldwise_init
struct L(Equatable, TrivialRegisterPassable, Writable):
    var v: Int

    def __eq__(self, other: Self) -> Bool:
        return self.v % 10 == other.v % 10

    def __neg__(self) -> Self:
        return L(-self.v - 100)

    def __getitem__(self, i: Int) -> Int:
        return self.v * i

    def __contains__(self, x: Int) -> Bool:
        return x < self.v

    def bump(mut self):
        self.v += 1

    def write_to(self, mut w: Some[Writer]):
        w.write("L<", self.v, ">")


@fieldwise_init
struct M(TrivialRegisterPassable, Writable):
    var l: L

    def write_to(self, mut w: Some[Writer]):
        w.write("M[", self.l, "]")


@fieldwise_init
struct Q(Copyable, Writable):
    var l: L
    var n: Int

    def write_to(self, mut w: Some[Writer]):
        w.write("Q(", self.l, ", ", self.n, ")")


@fieldwise_init
struct P(TrivialRegisterPassable):
    var p: UnsafePointer[Int, MutUntrackedOrigin]

    @no_inline
    def get(self) -> Int:
        return self.p[]

    @no_inline
    def step(mut self):
        self.p = self.p + 1


def make(n: Int) -> L:
    return L(n * 2)


@no_inline
def twice(x: L) -> L:
    return L(x.v * 2)


@no_inline
def show[T: Writable](x: T) -> String:
    return String(x)


def main():
    var a = L(3)
    var b = L(13)
    print(a, a == b, a != b, -a, a[4], 2 in a, 7 in a)
    a.bump()
    print(a, twice(a), show(a), String(a), repr(a.v))
    var m = M(L(5))
    m.l.bump()
    m.l.v += 10
    print(m, m.l.v)
    var q = Q(L(1), 2)
    q.l.v = 9
    print(q)
    ref r = q.l.v
    r += 1
    print(q.l)
    var p = unsafe_alloc[L](2)
    p[0] = L(20)
    p[1] = L(21)
    p[0].bump()
    p[1].v += 5
    print(p[0], p[1])
    p.unsafe_free()
    var xs = List[L]()
    xs.append(L(7))
    xs.append(L(8))
    xs[1].bump()
    print(xs[0], xs[1], len(xs))
    var o = Optional(L(30))
    print(o.value(), Bool(o))
    comptime c = make(4)
    print(c, c.v)
    var buf = unsafe_alloc[Int](2)
    buf[0] = 40
    buf[1] = 41
    var pp = P(buf)
    print(pp.get())
    pp.step()
    print(pp.get())
    buf.unsafe_free()
