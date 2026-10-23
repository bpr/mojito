# A raising initializer destroys the part of `self` it has built when it
# raises, as the pin does: each initialized field, in declaration order, when
# `self` is partly built, and `self` whole, through its `__deinit__`, once
# every field is set. A raise caught inside the initializer destroys nothing,
# and a raising call's error path destroys what a `raise` statement does.
# The partly built `self` dies before the locals dying with it, at the
# raising arm's entry. The inlined initializer behaves the same.


@fieldwise_init
struct Tracer(Movable):
    var name: String

    def __deinit__(deinit self):
        print("del", self.name)


def may(fail: Bool) raises -> Int:
    if fail:
        raise Error("callee")
    return 1


struct Three(Movable):
    var a: Tracer
    var b: Tracer
    var c: Tracer

    def __init__(out self, mode: Int) raises:
        self.b = Tracer("b")
        var loc = Tracer("loc")
        self.a = Tracer("a")
        if mode == 0:
            print("raising with", loc.name)
            raise Error("boom")
        _ = may(mode == 1)
        try:
            _ = may(mode == 2)
        except e:
            print("inner caught", e)
        self.c = Tracer("c")
        print("loc is", loc.name)
        if mode == 3:
            raise Error("full")

    def __deinit__(deinit self):
        print("del Three")


struct Pair(Movable):
    var a: Tracer
    var b: Tracer

    @always_inline
    def __init__(out self, fail: Bool) raises:
        self.a = Tracer("a")
        if fail:
            raise Error("inlined")
        self.b = Tracer("b")


def main():
    for mode in range(5):
        try:
            var t = Three(mode)
            print("made", t.a.name)
        except e:
            print("caught", e)
    for fail in [True, False]:
        try:
            var p = Pair(fail)
            print("made", p.b.name)
        except e:
            print("caught", e)
