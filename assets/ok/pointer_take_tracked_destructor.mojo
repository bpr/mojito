# A take through an origin-bearing pointer to an element with a
# destructor moves the value out as at the pin, and a write through the
# same place initializes it again; the local is destroyed once, with
# the written value.

from std.memory import Pointer

struct N(Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v
    def __init__(out self, *, deinit move: Self):
        self.v = move.v
    def __deinit__(deinit self):
        print("del", self.v)

def main():
    var a = N(1)
    var b = Pointer(to=a).unsafe_take_pointee()
    print("took", b.v)
    _ = b^
    Pointer(to=a).unsafe_write(N(2))
    print("end")
