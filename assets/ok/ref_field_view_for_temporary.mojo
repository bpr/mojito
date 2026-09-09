# A pointer-field view returned from an ordinary method drives a for-loop
# both as a temporary iterable and through a stored binding: the receiver
# loan flows through GetIter onto the loop iterator.
from std.iterable import Iterator, StopIteration

struct View[
    view_mut: Bool, //,
    view_origin: Origin[mut=view_mut],
](Copyable, Iterator):
    comptime Element = Int
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = View

    var src: Pointer[List[Int], Self.view_origin]
    var index: Int

    def __init__(out self, ref[Self.view_origin] src: List[Int], index: Int):
        self.src = Pointer(to=src)
        self.index = index

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    def __next__(mut self) raises StopIteration -> Int:
        if self.index >= len(self.src[]):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[][r]

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
        print(x)
    var v = b.view()
    for y in v:
        print(y)
