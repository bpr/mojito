# Field stores below a `List` subscript in every shape the store reaches: a
# member-base list, a `mut self` method looping over its own list, a `String`
# field's in-place dunder, a nested list, a field chain below the element, and
# a tuple unpack into a projected element target. The droppable-field twin
# (`conformance/fixtures/list_subscript_field_store_drop.mojo`) stays out of
# `assets/ok` until the native `List.append` temporary residue closes.
@fieldwise_init
struct Cell(Copyable, Movable):
    var n: Int
    var name: String

@fieldwise_init
struct Wrapper(Copyable, Movable):
    var inner: Cell

@fieldwise_init
struct Holder(Copyable, Movable):
    var items: List[Cell]

    def bump_all(mut self):
        for i in range(len(self.items)):
            self.items[i].n += 1

def main():
    var h = Holder([Cell(0, String("a")), Cell(10, String("b"))])
    h.items[1].n = 5
    h.items[0].name += "z"
    h.bump_all()
    print(h.items[0].n, h.items[1].n, h.items[0].name)

    var grid: List[List[Cell]] = [[Cell(0, String("g"))]]
    grid[0][0].n = 7
    print(grid[0][0].n)

    var ws: List[Wrapper] = [Wrapper(Cell(0, String("w")))]
    ws[0].inner.n = 8
    ws[0].inner.name = String("x")
    print(ws[0].inner.n, ws[0].inner.name)

    var k = 0
    ws[0].inner.n, k = 11, 12
    print(ws[0].inner.n, k)
