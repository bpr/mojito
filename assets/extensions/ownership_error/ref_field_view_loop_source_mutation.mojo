# expect: conflicts with live reference
# Mutating the source while looping over a method-returned ref-field view is
# rejected: the view's receiver loan stays live across the loop body.
@fieldwise_init
struct StopIteration:
    pass


@fieldwise_init
struct View[
    view_mut: Bool, //,
    view_origin: Origin[mut=view_mut],
](Copyable):
    comptime Element = Int
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = View

    var src: ref[view_origin] List[Int]
    var index: Int

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    def __next__(mut self) raises StopIteration -> Int:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]

struct Box:
    comptime ViewType[
        view_mut: Bool, //, view_origin: Origin[mut=view_mut]
    ] = View

    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(1)
        self.items.append(2)

    def view(ref self) -> Self.ViewType[origin_of(self)]:
        ref source = self.items
        return View(source, 0)

def main():
    var b = Box()
    for x in b.view():
        b.items.append(99)
        print(x)
