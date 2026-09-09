# A bare owned place auto-borrows into a pointer-storing view's ref ctor
# parameter: constructing a view no longer requires an explicit `ref` binding
# of the source first. Both an owned local and an owned field feed the ref
# slot directly, and the constructor stores `Pointer(to=...)` of it.
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def __init__(out self, ref[Self.o] src: List[Int], index: Int):
        self.src = Pointer(to=src)
        self.index = index

    def first(self) -> Int:
        return self.src[][self.index]

@fieldwise_init
struct Bag:
    var items: List[Int]

    def peek(self) -> Int:
        var v = View(self.items, 0)
        return v.first()

def main():
    var source = List[Int]()
    source.append(7)
    source.append(8)
    var v = View(source, 1)
    print(v.first())
    var items = List[Int]()
    items.append(4)
    var b = Bag(items^)
    print(b.peek())
