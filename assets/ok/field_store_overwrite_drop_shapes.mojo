# A store into an initialized droppable field destroys the value it replaces
# at the store, whatever reaches the field: a `mut self` method, a `mut`
# parameter, a nested field chain, or a constructor's second store into the
# same field. A constructor's first store initializes and destroys nothing,
# and a field moved out on one path is only destroyed where it still holds a
# value.
@fieldwise_init
struct Inner(Movable):
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Pair(Movable):
    var a: Inner
    var b: Inner

@fieldwise_init
struct Outer(Movable):
    var p: Pair
    var tag: Int

struct Holder(Movable):
    var item: Inner

    def __init__(out self, id: Int):
        self.item = Inner(id)

    def replace(mut self, id: Int):
        self.item = Inner(id)

struct Twice(Movable):
    var item: Inner

    def __init__(out self):
        self.item = Inner(40)
        self.item = Inner(41)

def overwrite_param(mut p: Pair, id: Int):
    p.a = Inner(id)

def take(var x: Inner):
    print("took", x.id)

def main():
    var h = Holder(1)
    print("built")
    h.replace(2)
    print("replaced")
    var p = Pair(Inner(10), Inner(11))
    overwrite_param(p, 12)
    print("param", p.a.id, p.b.id)
    var o = Outer(Pair(Inner(20), Inner(21)), 0)
    o.p.b = Inner(22)
    print("nested", o.p.a.id, o.p.b.id, o.tag)
    var q = Pair(Inner(30), Inner(31))
    if o.tag == 0:
        take(q.b^)
    q.b = Inner(32)
    print("conditional", q.a.id, q.b.id)
    var t = Twice()
    print("twice", t.item.id)
    print("end", h.item.id)
