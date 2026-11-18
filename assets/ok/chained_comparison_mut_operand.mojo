# Each link of a chained comparison binds its operands as its dunder's call
# does: a `mut` or `ref` parameter or receiver takes the operand's place,
# so its write lands in the caller's variable. The first middle operand is
# copied for the second link after the first link's write; a later middle
# is lent in place by both of its links.
@fieldwise_init
struct Bumped(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, *, copy: Self):
        print("copy", copy.v)
        self.v = copy.v

    def __lt__(self, mut o: Self) -> Bool:
        print("lt", self.v, o.v)
        o.v += 10
        return self.v < o.v


@fieldwise_init
struct Holder:
    var s: Bumped


@fieldwise_init
struct Counted(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, *, copy: Self):
        print("copy", copy.v)
        self.v = copy.v

    def __lt__(mut self, o: Self) -> Bool:
        print("lt", self.v, o.v)
        self.v += 1
        return self.v < o.v

    def __le__(self, o: Self) -> Bool:
        print("le", self.v, o.v)
        return self.v <= o.v


@fieldwise_init
struct Viewed(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, *, copy: Self):
        print("copy", copy.v)
        self.v = copy.v

    def __lt__(ref self, ref o: Self) -> Bool:
        print("lt", self.v, o.v)
        return self.v < o.v

    def __le__(ref self, ref o: Self) -> Bool:
        print("le", self.v, o.v)
        return self.v <= o.v


trait Bumpy(ImplicitlyCopyable, Deinitable):
    def __lt__(self, mut o: Self) -> Bool:
        ...


@fieldwise_init
struct Plain(Bumpy):
    var v: Int

    def __lt__(self, mut o: Self) -> Bool:
        print("lt", self.v, o.v)
        o.v += 10
        return self.v < o.v


def generic_chain[T: Bumpy](a: T, mut b: T, mut c: T) -> Bool:
    return a < b < c


def main():
    var k = Bumped(1)
    var holder = Holder(Bumped(2))
    var l = Bumped(30)
    print(k < holder.s < l)
    print(holder.s.v)

    var a = Bumped(1)
    var b = Bumped(2)
    var c = Bumped(3)
    var d = Bumped(40)
    print(a < b < c < d)
    print(a.v, b.v, c.v, d.v)

    var w = Counted(0)
    var x = Counted(2)
    var y = Counted(3)
    var z = Counted(40)
    print(w < x <= y < z)
    print(w.v, x.v, y.v, z.v)

    var p = Viewed(1)
    var q = Viewed(2)
    var r = Viewed(3)
    print(p < q <= r)

    var e = Plain(1)
    var f = Plain(2)
    var g = Plain(3)
    print(generic_chain(e, f, g))
    print(f.v, g.v)
