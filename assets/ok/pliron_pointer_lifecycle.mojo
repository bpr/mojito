# Compiler-private UnsafePointer storage intrinsics carry the VM's exact
# lifecycle natively: unsafe_write moves elements into raw slots, a
# take is a raw move out (no __copyinit__, no destructor), deinit runs the
# element destructor in place at the element offset, and free releases the
# allocation. The interleaved destructor prints pin the order — `taken`
# drops eagerly after its last read, so "deinit 1" lands before "took 1".
from std.memory import unsafe_alloc

struct Res(Movable, Deinitable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("deinit", self.id)

def main():
    var p = unsafe_alloc[Res](2)
    p.unsafe_write(Res(1))
    p.unsafe_offset(1).unsafe_write(Res(2))
    var taken = p.unsafe_take_pointee()
    print("took", taken.id)
    p.unsafe_offset(1).unsafe_deinit_pointee()
    p.unsafe_free()
    print("done")
