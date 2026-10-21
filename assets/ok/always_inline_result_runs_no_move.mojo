# An inlined callee's result reaches the caller with no move: the pin's
# inliner hands the returned value to the call's user, so neither a
# returned value nor an inlined initializer's `out self` runs
# `__init__(out self, *, deinit move: Self)`.


struct M(Movable):
    var v: Int

    @always_inline("nodebug")
    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, deinit move: Self):
        self.v = move.v
        print("move", self.v)


@always_inline("nodebug")
def make(x: Int) -> M:
    return M(x)


@always_inline("nodebug")
def pick(x: Int) -> M:
    if x > 0:
        return M(x)
    return M(-x)


def main():
    var a = M(1)
    print(a.v)
    var b = make(2)
    print(b.v)
    var c = pick(3)
    print(c.v)
    for _ in range(1):
        var d = make(4)
        print(d.v)
