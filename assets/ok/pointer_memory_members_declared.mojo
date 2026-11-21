# `Pointer`'s memory members are upstream's declared bodies: over a
# `Copyable` element whose copy, move, and destructor print, deinit,
# `copy=`, `unsafe_write_move_from`, a write of a call result, a write of
# a transfer, and a take each run the pin's copies, moves, and
# destructors.

from std.memory import Pointer

struct C(Copyable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
        print("copy", self.v)
    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1
        print("move", self.v)
    def __deinit__(deinit self):
        print("del", self.v)

def mk(v: Int) -> C:
    return C(v)

def main():
    var a = C(1)
    var b = C(2)
    var pa = Pointer(to=a)
    var pb = Pointer(to=b)
    print("-- deinit")
    pb.unsafe_deinit_pointee()
    print("-- copy")
    pb.unsafe_write(copy=a)
    print("-- deinit2")
    pb.unsafe_deinit_pointee()
    print("-- move_from")
    pb.unsafe_write_move_from(pa)
    print("-- write call result")
    pa.unsafe_write(mk(5))
    print("-- write var")
    var c = C(7)
    pa.unsafe_deinit_pointee()
    pa.unsafe_write(c^)
    print("-- take")
    var t = pa.unsafe_take_pointee()
    pa.unsafe_write(C(9))
    print("-- end", t.v)
