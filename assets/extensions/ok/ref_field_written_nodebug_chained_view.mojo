# A `nodebug` constructor whose `ref` argument is another view's `ref` field
# (`Pane(self.items, ...)`) is inlined with its parameter bound to the handle
# the field stores, the address the call passes, not to the field's own slot.
struct Pane[m: Bool, //, o: Origin[mut=m]]:
    var items: ref[o] List[Int]
    var start: Int

    @always_inline("nodebug")
    def __init__(out self, ref[Self.o] items: List[Int], start: Int):
        self.items = items
        self.start = start

    def shifted(self) -> Pane[o]:
        return Pane(self.items, self.start + 1)

    def first(self) -> Int:
        return self.items[self.start]

    def __getitem__(self, i: Int) -> Int:
        return self.items[self.start + i]

struct Board:
    comptime PaneType[m: Bool, //, o: Origin[mut=m]] = Pane

    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(5)
        self.items.append(6)
        self.items.append(7)

    def pane(ref self) -> Self.PaneType[origin_of(self)]:
        ref source = self.items
        return Pane(source, 0)

def main():
    var b = Board()
    print(b.pane().first())
    print(b.pane()[1])
    print(b.pane().shifted().first())
