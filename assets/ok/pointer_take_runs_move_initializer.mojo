# `__get_address_as_owned_value` transfers the pointee as a `var` would:
# `unsafe_write` of a take, and a take bound with `var`, each run one
# move initializer per move, as at the pin.

from std.memory import Pointer

struct M(Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1
        print("move", self.v)

def main():
    var a = M(11)
    var b = M(0)
    var pa = Pointer(to=a)
    var pb = Pointer(to=b)
    pb.unsafe_write(pa.unsafe_take_pointee())
    var x = pb.unsafe_take_pointee()
    print(x.v)
    pb.unsafe_write(M(1))
    pa.unsafe_write(M(2))
