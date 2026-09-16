# Every shape of a field store below a pointer-field dereference reaches the
# pointee: a local view, a method on a `MutOrigin` view, a view a call
# returns, a view held in a wrapper field, a nested field chain under the
# dereference, and a droppable field, which destroys the value it replaces at
# the store.
@fieldwise_init
struct Box:
    var v: Int

@fieldwise_init
struct Nest:
    var inner: Box

@fieldwise_init
struct Inner(Movable):
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Cell(Movable):
    var item: Inner

@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Box, Self.o]

@fieldwise_init
struct PM[o: MutOrigin]:
    var src: Pointer[Box, Self.o]

    def set(self, n: Int):
        self.src[].v = n

@fieldwise_init
struct PN[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Nest, Self.o]

@fieldwise_init
struct PC[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[Cell, Self.o]

@fieldwise_init
struct Wrap[m: Bool, //, o: Origin[mut=m]]:
    var view: P[Self.o]

def make(mut b: Box) -> P[origin_of(b)]:
    return P(Pointer(to=b))

def main():
    var b = Box(7)
    var p = P(Pointer(to=b))
    p.src[].v = 9
    print("direct", b.v)

    var bm = Box(0)
    var pm = PM(Pointer(to=bm))
    pm.set(10)
    print("method", bm.v)

    var b2 = Box(0)
    make(b2).src[].v = 11
    print("temporary", b2.v)

    var b3 = Box(0)
    var w = Wrap(P(Pointer(to=b3)))
    w.view.src[].v = 8
    print("wrapper", b3.v)

    var n = Nest(Box(0))
    var pn = PN(Pointer(to=n))
    pn.src[].inner.v = 12
    print("nested", n.inner.v)

    var c = Cell(Inner(1))
    var pc = PC(Pointer(to=c))
    pc.src[].item = Inner(2)
    print("droppable", c.item.id)
