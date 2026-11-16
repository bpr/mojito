# A `ref` field passed where its callee takes the operand by address passes
# the reference the field stores, as upstream's `lit.call` takes a `ref`
# field's `!lit.ref`: at a `mut` parameter, a register-passable one included,
# at a memory-only `read` parameter, as a `mut self` and a `self` receiver,
# and as the base of a subscript store and an augmented subscript store.

def bump(mut s: String):
    s += "!"


def show(s: String):
    print(s)


def inc(mut x: Int):
    x += 1


struct Box:
    var s: String

    def __init__(out self, s: String):
        self.s = s

    def grow(mut self):
        self.s += "+"

    def get(self) -> String:
        return self.s


struct Cell:
    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(1)
        self.items.append(2)

    def __getitem__(self, i: Int) -> Int:
        return self.items[i]

    def __setitem__(mut self, i: Int, v: Int):
        self.items[i] = v


@fieldwise_init
struct Holder[o: MutOrigin]:
    var r: ref[Self.o] String


@fieldwise_init
struct IntHolder[o: MutOrigin]:
    var r: ref[Self.o] Int


@fieldwise_init
struct BoxHolder[o: MutOrigin]:
    var r: ref[Self.o] Box


@fieldwise_init
struct CellHolder[o: MutOrigin]:
    var r: ref[Self.o] Cell


def main():
    var s = String("a")
    var h = Holder(s)
    bump(h.r)
    show(h.r)
    var n = 4
    var ih = IntHolder(n)
    inc(ih.r)
    print(n)
    var b = Box("b")
    var bh = BoxHolder(b)
    bh.r.grow()
    print(bh.r.get())
    var c = Cell()
    var ch = CellHolder(c)
    ch.r[1] = 7
    ch.r[0] += 5
    print(c[0], c[1])
    print(s)
