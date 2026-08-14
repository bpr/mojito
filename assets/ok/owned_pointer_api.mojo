# The minimal OwnedPointer proof subset with current Mojo's naming from day
# one: value and `init_with=` placement construction, `into_inner` (never the
# pre-rename `take`), interior-origin `unsafe_ptr()` views, and a conditional
# destructor (a linear pointee makes the OwnedPointer itself linear).
from std.memory import OwnedPointer, Allocation, Layout, alloc, dealloc

struct Res(Movable):
    var id: Int
    def __init__(out self, id: Int):
        self.id = id
    def __init__(out self, *, deinit move: Self):
        self.id = move.id
    def __deinit__(deinit self):
        print("drop", self.id)

def main():
    var p = OwnedPointer[Int](41)
    var view = p.unsafe_ptr()
    view[0] += 1
    print("deref", view[0])
    print("inner", p^.into_inner())

    var base = 6
    var q = OwnedPointer[Int](init_with=lambda () -> Int: base + 1)
    var qview = q.unsafe_ptr()
    print("placed", qview[0])

    # implicit drop runs the pointee destructor exactly once
    var r = OwnedPointer[Res](Res(9))
    var rview = r.unsafe_ptr()
    print("held", rview[0].id)
    print("done")
