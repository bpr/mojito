# A drop of a value whose `__deinit__` is `@always_inline("nodebug")` holds
# the destructor's body, its owner's parameters bound to the dropped type's,
# where the value dies whole: after a last read, at the end of a loop
# iteration, on the path a conditional move skips, before a reassignment,
# as a field of a struct with no destructor of its own, as a field a
# store overwrites, through a `mut self` too, and in an `except` handler
# entered while it lives; a field ended explicitly through a `mut self`
# runs its named destructor in place.


struct Probe[k: Int](Movable):
    var v: Int
    var tag: String

    def __init__(out self, v: Int):
        self.v = v
        self.tag = "t" + String(v)

    @always_inline("nodebug")
    def __deinit__(deinit self):
        print("end", Self.k, self.v, self.tag)

    def finish(deinit self):
        print("finish", Self.k, self.v)


def sink(var p: Probe[1]):
    print("sink", p.v)


def maybe_sink(c: Bool):
    var p = Probe[1](1)
    if c:
        sink(p^)
    print("after", c)


def generic[n: Int](x: Int) -> Int:
    var p = Probe[n](x)
    return p.v + n


def owned(var p: Probe[2]):
    print("owned", p.v)


struct Holder[k: Int](Movable):
    var probe: Probe[Self.k]
    var n: Int

    def __init__(out self, v: Int):
        self.probe = Probe[Self.k](v)
        self.n = v

    def reset(mut self, v: Int):
        self.probe = Probe[Self.k](v)

    def renew(mut self, v: Int):
        self.probe^.finish()
        self.probe = Probe[Self.k](v)


def held():
    var h = Holder[6](10)
    print("held", h.n, h.probe.v)
    h.probe = Probe[6](11)
    print("replaced", h.probe.v)
    h.reset(13)
    print("reset", h.probe.v)
    h.renew(14)
    print("renewed", h.probe.v)


def may_raise(c: Bool) raises:
    if c:
        raise Error("boom")


def caught():
    var p = Probe[7](12)
    try:
        may_raise(True)
    except e:
        print("caught", p.v)
    print("after caught")


def main():
    print(generic[5](1))
    for i in range(2):
        var p = Probe[3](i)
        print("iter", p.v)
    maybe_sink(True)
    maybe_sink(False)
    owned(Probe[2](7))
    var p = Probe[4](8)
    print(p.tag)
    p = Probe[4](9)
    print(p.tag)
    held()
    caught()
