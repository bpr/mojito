# A discarded transfer (`_ = x^`, or `x^` as a statement) destroys `x` where
# it lies, with no move initializer, over a local, a `var` parameter, or a
# `mut` parameter; only a binding (`var c = b^`) moves into new storage.


struct M(Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
        print("copy", self.v)

    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1000
        print("move", self.v)

    def __deinit__(deinit self):
        print("del", self.v)


def own(var p: M):
    _ = p^
    print("own done")


def reset(mut p: M):
    _ = p^
    p = M(9)


def fail() raises:
    raise Error("boom")


def main():
    var a = M(50)
    _ = a^
    print("after local")

    var b = M(60)
    var c = b^
    _ = c^
    print("after bound")

    var d = M(70)
    if True:
        _ = d^
    print("after arm")

    for i in range(2):
        var e = M(i)
        _ = e^
    print("after loop")

    own(M(3))

    var f = M(40)
    reset(f)
    print("reset", f.v)

    var g = M(55)
    g^
    print("after statement")

    try:
        var h = M(80)
        _ = h^
        fail()
    except:
        print("caught")
