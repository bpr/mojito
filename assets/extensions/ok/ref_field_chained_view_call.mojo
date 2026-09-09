# A method (or subscript) chained directly onto a temporary borrowing view
# retains the view in a hidden slot whose loans keep the source alive across
# the chained call — no intermediate `var` binding required. A view-to-view
# method chains one level deeper the same way.
@fieldwise_init
struct Pane[m: Bool, //, o: Origin[mut=m]]:
    var items: ref[o] List[Int]
    var start: Int

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
