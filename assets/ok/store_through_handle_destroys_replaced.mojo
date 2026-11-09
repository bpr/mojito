# An assignment through a handle that names no owner place destroys the
# value it replaces first: a reassigned pointer, a cast pointer, a
# destructured pointer, a tuple element, a `__getitem__` reference with no
# `__setitem__`, a `ref` binding of a call result, and a method's returned
# reference.


struct Inner(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __del__(deinit self):
        print("del", self.v)


struct Holder(Movable):
    var j: Inner

    def __init__(out self, v: Int):
        self.j = Inner(v)

    def __getitem__(ref self, i: Int) -> ref[self.j] Inner:
        return self.j


def reassigned_pointer():
    var b = Inner(1)
    var q = Pointer(to=b)
    q = Pointer(to=b)
    q[] = Inner(2)
    print("after", b.v)


def cast_pointer():
    var h = Holder(30)
    var r = Pointer(to=h.j).unsafe_origin_cast[origin_of(h)]()
    r[] = Inner(31)
    print("after", h.j.v)


def destructured_pointer():
    var a = Inner(40)
    var (p, x) = (Pointer(to=a), 1)
    p[] = Inner(41)
    print("after", a.v, x)


def tuple_element():
    var t = (Inner(60), Inner(61))
    t[0] = Inner(62)
    print("after", t[0].v, t[1].v)


def getitem_reference():
    var b = Holder(1)
    b[0] = Inner(5)
    print("after", b.j.v)


def ref_binding():
    var b = Holder(7)
    ref r = b[0]
    r = Inner(8)
    print("after", b.j.v)


def main():
    reassigned_pointer()
    cast_pointer()
    destructured_pointer()
    tuple_element()
    getitem_reference()
    ref_binding()
