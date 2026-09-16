# A whole-value write through a reference destroys the value it replaces,
# whatever reference carries it: `self` in a `mut self` method, a `mut`
# parameter, the same parameter twice in one body, or a whole-variable `ref`
# binding. Each replaced value belongs to the caller or to another slot, so
# none of them has a redefining `DefVar` to end its live range.
@fieldwise_init
struct Inner(Movable):
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Pair(Movable):
    var a: Inner
    var b: Inner

struct Holder(Movable):
    var item: Inner

    def __init__(out self, id: Int):
        self.item = Inner(id)

    def reset(mut self, id: Int):
        self = Holder(id)

def reset_param(mut p: Pair):
    p = Pair(Inner(7), Inner(8))

def twice(mut p: Pair):
    p = Pair(Inner(70), Inner(71))
    p = Pair(Inner(72), Inner(73))

def main():
    var h = Holder(1)
    h.reset(2)
    print("self-reassigned", h.item.id)
    var p = Pair(Inner(10), Inner(11))
    reset_param(p)
    print("param", p.a.id)
    var q = Pair(Inner(20), Inner(21))
    twice(q)
    print("twice", q.a.id)
    var x = Inner(30)
    ref r = x
    r = Inner(31)
    print("ref-rebind", x.id)
