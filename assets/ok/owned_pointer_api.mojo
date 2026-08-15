# The minimal OwnedPointer proof subset with current Mojo's naming: value
# construction, `into_inner` (never the pre-rename `take`), interior-origin
# `unsafe_ptr()` views, and a conditional destructor (a linear pointee makes
# the OwnedPointer itself linear). Upstream's borrowed `p[]` dereference and
# `init_with=`/`copy_value=` constructors are recorded gaps.
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

    # implicit drop runs the pointee destructor exactly once
    var r = OwnedPointer[Res](Res(9))
    var rview = r.unsafe_ptr()
    print("held", rview[0].id)
    print("done")
