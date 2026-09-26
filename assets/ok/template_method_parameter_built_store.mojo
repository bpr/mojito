# A per-instantiation method clone inherits its checked template's facts when
# an augmented element store reads through a getter the instance retargets,
# or updates an element of a bare parameter type through its bound's
# in-place dunder (`docs/notes/instantiation-from-template.md`, feature
# `subscript_stores`): the value getter of a subscripted value built over
# the parameter (`self.box[i] += 1`, `self[i] += 2`) is realized on the
# instance's own receiver as its setter is, and an `__iadd__` dispatched
# through `T`'s bound is re-selected on the instance's element type. A plain
# `a += b` on a bound `T` dispatches through the bound the same way.
trait Accum:
    def __iadd__(mut self, rhs: Self):
        ...


struct Meter(Accum, ImplicitlyCopyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __iadd__(mut self, rhs: Self):
        self.n += rhs.n


struct Gauge(Accum, ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __iadd__(mut self, rhs: Self):
        self.v += rhs.v * 10


# A value getter and a setter on a struct built over the parameter.
struct Box[T: ImplicitlyCopyable & Deinitable](Movable):
    var first: Int
    var second: Int
    var tags: List[Self.T]

    def __init__(out self):
        self.first = 0
        self.second = 0
        self.tags = List[Self.T]()

    def __getitem__(self, i: Int) -> Int:
        if i == 0:
            return self.first
        return self.second

    def __setitem__(mut self, i: Int, value: Int):
        if i == 0:
            self.first = value
        else:
            self.second = value


# A value getter and a setter whose element is the parameter itself.
struct Duo[T: Accum & ImplicitlyCopyable & Deinitable](Movable):
    var first: Self.T
    var second: Self.T

    def __init__(out self, first: Self.T, second: Self.T):
        self.first = first
        self.second = second

    def __getitem__(self, i: Int) -> Self.T:
        if i == 0:
            return self.first
        return self.second

    def __setitem__(mut self, i: Int, value: Self.T):
        if i == 0:
            self.first = value
        else:
            self.second = value


struct Rack[T: Accum & ImplicitlyCopyable & Deinitable]:
    var box: Box[Self.T]
    var items: List[Self.T]
    var duo: Duo[Self.T]
    var count: Int

    def __init__(out self, a: Self.T, b: Self.T):
        self.box = Box[Self.T]()
        self.items = List[Self.T]()
        self.items.append(a)
        self.items.append(b)
        self.duo = Duo[Self.T](a, b)
        self.count = 0

    def __getitem__(self, i: Int) -> Int:
        return self.count

    def __setitem__(mut self, i: Int, value: Int):
        self.count = value

    def bump_box(mut self, i: Int):
        self.box[i] += 1

    def bump_self(mut self, i: Int):
        self[i] += 2

    def accumulate(mut self, i: Int, x: Self.T):
        self.items[i] += x

    def fold(mut self, i: Int, x: Self.T):
        self.duo[i] += x

    def item(self, i: Int) -> Self.T:
        return self.items[i]

    def pair(self, i: Int) -> Self.T:
        return self.duo[i]


def add_in[T: Accum & ImplicitlyCopyable](mut a: T, b: T):
    a += b


def main():
    var meters = Rack[Meter](Meter(1), Meter(2))
    meters.bump_box(1)
    meters.bump_box(1)
    meters.bump_self(0)
    meters.accumulate(0, Meter(5))
    meters.fold(1, Meter(7))
    print(meters.box[1], meters[0], meters.item(0).n, meters.pair(1).n)
    var gauges = Rack[Gauge](Gauge(1), Gauge(2))
    gauges.bump_box(0)
    gauges.bump_self(0)
    gauges.bump_self(0)
    gauges.accumulate(1, Gauge(3))
    gauges.fold(0, Gauge(4))
    print(gauges.box[0], gauges[0], gauges.item(1).v, gauges.pair(0).v)
    var m = Meter(1)
    add_in(m, Meter(2))
    var g = Gauge(1)
    add_in(g, Gauge(2))
    print(m.n, g.v)
