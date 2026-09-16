# A store into an initialized droppable field destroys the value it replaces
# at the store, as the pinned Mojo does: `del 2` at `p.b = Inner(3)`, then
# `del 1` and `del 3` when `p` dies after its last use, then `after`.
@fieldwise_init
struct Inner:
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Pair:
    var a: Inner
    var b: Inner

def main():
    var p = Pair(Inner(1), Inner(2))
    p.b = Inner(3)
    print("after")
