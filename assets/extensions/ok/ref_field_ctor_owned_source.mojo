# A bare owned place auto-borrows into a ref ctor parameter: constructing a
# view no longer requires an explicit `ref` binding of the source first. Both
# an owned local and an owned field feed the ref slot directly.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def first(self) -> Int:
        return self.src[self.index]

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
