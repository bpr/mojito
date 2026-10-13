# A reference result through a read parameter borrows the caller's
# argument: a named local, a field, a list element, or a temporary the
# result keeps alive, across free calls, methods, statics, and keywords.
# The borrow holds through function values and nested defs, raising or not.


struct W(Copyable):
    var t: Int
    var s: String

    def __init__(out self, t: Int):
        self.t = t
        self.s = String("w")

    def __deinit__(deinit self):
        print("del", self.t)


struct Box[T: Copyable & Writable & Deinitable](Copyable):
    var t: Self.T
    var s: String

    def __init__(out self, var t: Self.T):
        self.t = t^
        self.s = String("box")


struct O(Movable):
    var w: W

    def __init__(out self, t: Int):
        self.w = W(t)


struct H:
    def __init__(out self):
        pass

    def pick(self, w: W) -> ref[origin_of(w.t)] Int:
        return w.t

    @staticmethod
    def spick(w: W) -> ref[origin_of(w.t)] Int:
        return w.t


def field(w: W) -> ref[origin_of(w.t)] Int:
    return w.t


def whole[T: Copyable & Writable & Deinitable](b: Box[T]) -> ref[origin_of(b)] Box[T]:
    return b


def keyword(*, w: W) -> ref[origin_of(w.t)] Int:
    return w.t


def raising(w: W) raises -> ref[origin_of(w.t)] Int:
    return w.t


def main() raises:
    var w = W(1)
    print(field(w))
    var o = O(2)
    print(field(o.w))
    var l = List[W]()
    l.append(W(3))
    print(field(l[0]))
    print(field(W(4)))
    ref r = field(W(5))
    print(r)
    var b = Box(8)
    print(whole(b).t)
    print(keyword(w=w))
    var h = H()
    print(h.pick(w))
    print(h.pick(W(9)))
    print(H.spick(w))
    var f = field
    print(f(w))
    print(f(W(10)))

    def inner(w: W) -> ref[origin_of(w.t)] Int:
        return w.t

    print(inner(w))
    print(inner(W(11)))
    var g = raising
    print(g(w))
    print(g(W(12)))
    print("end")
