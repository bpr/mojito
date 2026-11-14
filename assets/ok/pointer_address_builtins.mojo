# Upstream's address builtins: `__get_litref_as_mvalue(r)` is the place a
# `!lit.ref` refers to, and `__get_address_as_owned_value(k)` moves the
# value a `!kgen.pointer` holds out of it.
from std.memory import Layout, dealloc


struct N(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


struct C(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)


def inc(mut v: Int):
    v += 10


def show(c: C):
    print("show", c.v)


def touch[o: MutOrigin](r: Pointer[Int, o]._mlir_lit_ref):
    inc(__get_litref_as_mvalue(r))
    ref y = __get_litref_as_mvalue(r)
    y += 100
    __get_litref_as_mvalue(r) = __get_litref_as_mvalue(r) * 2


def read[o: ImmOrigin](r: Pointer[Int, o]._mlir_lit_ref) -> ref[o] Int:
    return __get_litref_as_mvalue(r)


def look[o: ImmOrigin](r: Pointer[C, o]._mlir_lit_ref):
    show(__get_litref_as_mvalue(r))
    print(__get_litref_as_mvalue(r).v)
    var d = __get_litref_as_mvalue(r).copy()
    print(d.v)


def main():
    var x = 1
    touch(__get_mvalue_as_litref(x))
    print(x)
    var r = __get_mvalue_as_litref(x)
    print(read(r), x)
    var c = C(5)
    look(__get_mvalue_as_litref(c))
    var a = alloc(Layout[N](count=1))
    var q = a.unsafe_ptr()
    q.unsafe_write(N(7))
    var n = __get_address_as_owned_value(q._get_kgen_pointer())
    print(n.v)
    dealloc(a^)
    var t = (N(1), N(2))

    def handler[idx: Int](var elt: t.Ts[idx]):
        var m = rebind_var[N](elt^)
        print("n", m.v)

    t^.consume_elements(handler)
    print("end")
