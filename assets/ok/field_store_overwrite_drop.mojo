# A store into an initialized droppable field replaces a value that dies at
# its last use, as the pinned Mojo destroys it: `p.b`'s first value is never
# read, so `del 2` runs as soon as `p` is built, then `del 1` and `del 3`
# when `p` dies after the store, its last use, then `after`.
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
