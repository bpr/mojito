# The OwnedPointer surface with current Mojo's naming: by-value, copying
# (`copy_value=`, a borrowed non-implicitly-copyable value, `other=`)
# construction, the `p[]` dereference, `ptr()`/`unsafe_ptr()` views,
# `into_inner` (never the pre-rename `take`), and a conditional destructor (a
# linear pointee makes the OwnedPointer itself linear).
from std.memory import OwnedPointer, Allocation, Layout, alloc, dealloc

struct Res(Copyable):
    var id: Int
    def __init__(out self, id: Int):
        self.id = id
    def __init__(out self, *, copy: Self):
        print("copy", copy.id)
        self.id = copy.id + 100
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

    var c = OwnedPointer(copy_value=String("hi"))
    var d = OwnedPointer(other=c)
    d[] += "!"
    print(c[], d[])
    var res = Res(1)
    var borrowed = OwnedPointer(res)
    borrowed[].id += 1
    print("borrowed", borrowed[].id, res.id)
    var n = 3
    var by_value = OwnedPointer(n)
    by_value[] += 1
    print(by_value.ptr()[], n)
