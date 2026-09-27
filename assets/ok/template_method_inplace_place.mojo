# A per-instantiation method clone inherits its checked template's facts when
# the method updates a place through its in-place dunder
# (`docs/notes/instantiation-from-template.md`, feature `statements`): a
# field of the bare parameter type updates through its bound's `__iadd__`,
# re-selected on the instance's type, and a field of a closed struct or of
# one built over the parameter through that struct's own dunder, the
# instance's clone where it has one, which may raise in a method declared
# `raises`. A `mut` parameter or a `var` local updates the same way.
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


struct Strict(ImplicitlyCopyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __iadd__(mut self, rhs: Self) raises:
        if rhs.n < 0:
            raise "negative"
        self.n += rhs.n


# A struct built over the parameter, whose dunder takes a closed operand.
struct Tick[T: ImplicitlyCopyable & Deinitable](ImplicitlyCopyable):
    var count: Int

    def __init__(out self):
        self.count = 0

    def __iadd__(mut self, k: Int):
        self.count += k


struct Holder(ImplicitlyCopyable):
    var meter: Meter

    def __init__(out self):
        self.meter = Meter(0)


struct Rack[T: Accum & ImplicitlyCopyable & Deinitable]:
    var total: Self.T
    var meter: Meter
    var tick: Tick[Self.T]
    var inner: Holder
    var strict: Strict

    def __init__(out self, start: Self.T):
        self.total = start
        self.meter = Meter(0)
        self.tick = Tick[Self.T]()
        self.inner = Holder()
        self.strict = Strict(0)

    def add(mut self, x: Self.T):
        self.total += x

    def bump_meter(mut self):
        self.meter += Meter(1)

    def bump_meter_by(mut self, k: Int):
        self.meter += Meter(k)

    def bump_tick(mut self, k: Int):
        self.tick += k

    def bump_inner(mut self):
        self.inner.meter += Meter(2)

    def bump_checked(mut self, k: Int) raises:
        self.strict += Strict(k)

    def absorb(self, mut into: Self.T, x: Self.T):
        into += x

    def bumped(self, k: Int) -> Int:
        var m = self.meter
        m += Meter(k)
        return m.n

    def summed(self, x: Self.T) -> Self.T:
        var t = self.total
        t += x
        return t

    def result(self) -> Self.T:
        return self.total


def main() raises:
    var meters = Rack[Meter](Meter(1))
    meters.add(Meter(5))
    meters.bump_meter()
    meters.bump_meter_by(4)
    meters.bump_tick(3)
    meters.bump_tick(3)
    meters.bump_inner()
    var m = Meter(10)
    meters.absorb(m, Meter(7))
    meters.bump_checked(8)
    print(meters.result().n, meters.meter.n, meters.tick.count, meters.inner.meter.n, m.n)
    print(meters.bumped(3), meters.meter.n, meters.summed(Meter(2)).n, meters.result().n)
    try:
        meters.bump_checked(-1)
    except e:
        print(e, meters.strict.n)
    var gauges = Rack[Gauge](Gauge(1))
    gauges.add(Gauge(2))
    gauges.bump_meter()
    gauges.bump_tick(9)
    gauges.bump_inner()
    gauges.bump_inner()
    var g = Gauge(1)
    gauges.absorb(g, Gauge(3))
    gauges.bump_checked(2)
    gauges.bump_checked(3)
    print(gauges.strict.n, gauges.result().v, gauges.meter.n, gauges.tick.count, gauges.inner.meter.n, g.v)
    print(gauges.bumped(4), gauges.meter.n, gauges.summed(Gauge(2)).v, gauges.result().v)
