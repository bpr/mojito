# A droppable field replaced through a `List` subscript is destroyed at the
# store (`del 1` at `xs[0].tag = Tracked(2)`), and so is one below a nested
# field chain, exactly as the pinned Mojo prints.
@fieldwise_init
struct Tracked(Copyable, Movable):
    var id: Int

    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Slot(Copyable, Movable):
    var tag: Tracked
    var n: Int

@fieldwise_init
struct Wrapper(Copyable, Movable):
    var inner: Slot

def main():
    var xs = List[Slot]()
    xs.append(Slot(Tracked(1), 0))
    xs[0].n = 5
    xs[0].tag = Tracked(2)
    print("replaced")
    var ws = List[Wrapper]()
    ws.append(Wrapper(Slot(Tracked(3), 0)))
    ws[0].inner.tag = Tracked(4)
    print("nested")
