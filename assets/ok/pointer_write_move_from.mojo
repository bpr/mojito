# `Pointer.unsafe_write_move_from` takes the source pointee and moves it into
# the receiver's slot with exactly one move initializer, as upstream's
# `__get_address_as_uninit_lvalue(self) = __get_address_as_owned_value(src)`.
from std.memory.alloc import unsafe_alloc

struct M(Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1
        print("move", self.v)

def main():
    var pa = unsafe_alloc[M](1)
    var pb = unsafe_alloc[M](1)
    var m = M(10)
    pa.unsafe_write(m^)
    print("-- move_from")
    pb.unsafe_write_move_from(pa)
    print("-- end", pb[].v)
    pb.unsafe_deinit_pointee()
    pa.unsafe_free()
    pb.unsafe_free()
