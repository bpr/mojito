# expect: conflicts with live reference
# The hidden slot retaining a chained temporary view carries the source loan:
# an argument in the same statement that mutates the source conflicts.
@fieldwise_init
struct Pane[m: Bool, //, o: Origin[mut=m]]:
    var items: ref[o] List[Int]
    var start: Int

    def at(self, i: Int) -> Int:
        return self.items[i]

struct Board:
    comptime PaneType[m: Bool, //, o: Origin[mut=m]] = Pane

    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(5)

    def pane(ref self) -> Self.PaneType[origin_of(self)]:
        ref source = self.items
        return Pane(source, 0)

    def poke(mut self) -> Int:
        self.items.append(9)
        return 0

def main():
    var b = Board()
    print(b.pane().at(b.poke()))
