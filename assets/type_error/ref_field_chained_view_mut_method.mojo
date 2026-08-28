# expect: invalid assignment target: the temporary result of method call 'pane()'
# Retaining a chained temporary view licenses read-only use; a `mut self`
# method on the temporary stays rejected.
@fieldwise_init
struct Pane[m: Bool, //, o: Origin[mut=m]]:
    var items: ref[o] List[Int]
    var start: Int

    def advance(mut self):
        self.start += 1

struct Board:
    comptime PaneType[m: Bool, //, o: Origin[mut=m]] = Pane

    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(5)

    def pane(ref self) -> Self.PaneType[origin_of(self)]:
        ref source = self.items
        return Pane(source, 0)

def main():
    var b = Board()
    b.pane().advance()
