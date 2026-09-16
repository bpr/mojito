# A store through a pointer dereference destroys the pointee it replaces at the
# store, as the pinned Mojo does: `del 1`, then `after 2`, then `del 2`. The
# replaced value belongs to another slot, so no redefining `DefVar` ends its
# live range; drop elaboration splices a `DropPlace` before the write, whose
# place is the owner MIR substituted for the stably bound pointer.
@fieldwise_init
struct Inner(Movable):
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

def main():
    var b = Inner(1)
    var q = Pointer(to=b)
    q[] = Inner(2)
    print("after", b.id)
