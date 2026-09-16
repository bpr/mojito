# Reassigning a `mut` parameter as a whole destroys the caller's old value at
# the assignment, as the pinned Mojo does: `del 1` and `del 2` before `7`.
# The replaced value lives in the caller's storage, so no redefining `DefVar`
# ends its live range; drop elaboration splices a `DropPlace` before the write
# through the parameter's own handle.
@fieldwise_init
struct Inner:
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Pair:
    var a: Inner
    var b: Inner

def reset(mut p: Pair):
    p = Pair(Inner(7), Inner(8))

def main():
    var p = Pair(Inner(1), Inner(2))
    reset(p)
    print(p.a.id)
