# A pointer subscript reads its pointee in place, as upstream's
# `ref`-returning `UnsafePointer.__getitem__` does: a copy is made only
# where the value is consumed, and exactly once.


struct S(Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 10
        print("copy S", copy.v)

    def __del__(deinit self):
        print("del S", self.v)


struct C(ImplicitlyCopyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 10
        print("copy C", copy.v)

    def __del__(deinit self):
        print("del C", self.v)


struct H(Movable):
    var c: C

    def __init__(out self, var c: C):
        self.c = c^


def generic_copy[T: Copyable & Movable & Deinitable](p: UnsafePointer[T, _]) -> T:
    return p[0].copy()


def concrete_copy(p: UnsafePointer[S, _]) -> S:
    return p[0].copy()


def deref_copy(p: UnsafePointer[S, _]) -> S:
    return p[].copy()


def keyword_copy(p: UnsafePointer[S, _]) -> S:
    return p[unsafe_offset=0].copy()


def returned(p: UnsafePointer[C, _]) -> C:
    return p[0]


def take(var c: C):
    print("take", c.v)


def main():
    var ls: List[S] = [S(4)]
    var lc: List[C] = [C(5)]
    var ps = ls.unsafe_ptr()
    var pc = lc.unsafe_ptr()

    print("generic copy")
    var g = generic_copy(ps)
    print("g", g.v)
    print("concrete copy")
    var c = concrete_copy(ps)
    print("c", c.v)
    print("deref copy")
    var d = deref_copy(ps)
    print("d", d.v)
    print("keyword copy")
    var k = keyword_copy(ps)
    print("k", k.v)
    print("field read")
    print("v", ps[0].v)
    print("call result copy")
    var r = ls.unsafe_ptr()[0].copy()
    print("r", r.v)

    print("implicit copy")
    var x = pc[0]
    print("x", x.v)
    print("returned")
    var y = returned(pc)
    print("y", y.v)
    print("var argument")
    take(pc[0])
    print("tuple element")
    var t = (pc[0], 1)
    print("t", t[0].v)
    print("constructor argument")
    var h = H(pc[0])
    print("h", h.c.v)

    print("place pointer")
    var a = C(6)
    var pa = Pointer(to=a)
    var z = pa[]
    print("z", z.v)
    print("a", a.v)

    print("slice")
    var m = ls[0:1]
    print("m", m[0].v)
    print("lists", len(ls), len(lc))
