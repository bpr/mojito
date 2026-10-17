# Nested `def`s marked `@always_inline("nodebug")` whose closures their
# callers never show: a copied capture snapshotted before a later
# assignment, a moved capture whose destructor prints, a copied capture
# taken afresh on each trip of a loop, a call through a captured sibling,
# a body declared in a `comptime for`, and a capturing closure passed as a
# callee's callable parameter.


struct Noisy(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("del", self.tag)


def copied(x: Int) -> Int:
    var y = x

    @always_inline("nodebug")
    def inc() {var y} -> Int:
        return y + 1

    y = 100
    return inc() + y


def moved() -> Int:
    var n = Noisy(7)

    @always_inline("nodebug")
    def tag() {n^} -> Int:
        return n.tag * 2

    print("before")
    var result = tag()
    print("after")
    return result


struct Counted(ImplicitlyCopyable, Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __init__(out self, *, copy: Self):
        self.tag = copy.tag + 100
        print("copy", self.tag)

    def __deinit__(deinit self):
        print("del", self.tag)


def looped():
    var i = 0
    while i < 2:
        var c = Counted(i)

        @always_inline("nodebug")
        def show() {var c} -> Int:
            return c.tag

        print("call", show(), show())
        i += 1


def sibling(k: Int) -> Int:
    @always_inline("nodebug")
    def get() {imm k} -> Int:
        return k

    @always_inline("nodebug")
    def twice() {get} -> Int:
        return get() + get()

    return twice()


def in_loop() -> Int:
    var s = 0
    comptime for i in range(4):

        @always_inline("nodebug")
        def add() {mut s}:
            s += i * 10

        add()
    return s


@always_inline("nodebug")
def apply[
    origins: OriginSet, //, f: def(Int) capturing[origins] -> Int
](x: Int) -> Int:
    return f(x)


def binder(k: Int) -> Int:
    @always_inline("nodebug")
    @parameter
    def plus(y: Int) -> Int:
        return y + k

    return apply[plus](3)


def main():
    print(copied(4))
    print(moved())
    looped()
    print(sibling(5))
    print(in_loop())
    print(binder(4))
