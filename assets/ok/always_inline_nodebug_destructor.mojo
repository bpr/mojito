# A drop of a value whose `__deinit__` is `@always_inline("nodebug")` holds
# the destructor's body, its owner's parameters bound to the dropped type's,
# where the value dies whole: after a last read, at the end of a loop
# iteration, on the path a conditional move skips, and before a
# reassignment.


struct Probe[k: Int](Movable):
    var v: Int
    var tag: String

    def __init__(out self, v: Int):
        self.v = v
        self.tag = "t" + String(v)

    @always_inline("nodebug")
    def __deinit__(deinit self):
        print("end", Self.k, self.v, self.tag)


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
