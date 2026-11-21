# Upstream's `__get_address_as_uninit_lvalue(k) = value` initializes the
# place a `!kgen.pointer` addresses: the value is built for the slot, so it
# need not be `Movable`, and whatever the slot held is not destroyed first.
from std.memory import Layout, dealloc


struct Pinned:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


struct N(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


def heap():
    var a = alloc(Layout[N](count=1))
    var q = a.unsafe_ptr()
    __get_address_as_uninit_lvalue(q._mlir_value) = N(1)
    print(q[].v)
    var n = __get_address_as_owned_value(q._get_kgen_pointer())
    print(n.v)
    dealloc(a^)


def main():
    heap()
    var x = 5
    __get_address_as_uninit_lvalue(Pointer(to=x)._mlir_value) = 6
    print(x)
    var p = Pinned(2)
    __get_address_as_uninit_lvalue(Pointer(to=p)._mlir_value) = Pinned(3)
    print(p.v)
    var t = (1, String("a"))
    __get_address_as_uninit_lvalue(Pointer(to=t[1])._mlir_value) = String("b")
    print(t[0], t[1])
