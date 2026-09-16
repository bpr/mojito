# Copying a struct that has no copy constructor of its own copies each field
# through the field's own copy constructor, so a heap-backed `Optional`, an
# owning collection and a user field with a printing `__copyinit__` each end
# up with their own storage, and both copies are destroyed independently.
struct Loud(Copyable, Movable):
    var tag: Int
    var items: List[Int]

    def __init__(out self, n: Int):
        self.tag = n
        self.items = List[Int]()
        self.items.append(n)

    def __init__(out self, *, copy: Self):
        print("copy", copy.tag)
        self.tag = copy.tag
        self.items = copy.items.copy()

    def __deinit__(deinit self):
        print("del", self.tag)

@fieldwise_init
struct Entry[K: Copyable & Deinitable, V: Copyable & Deinitable](Copyable, Movable):
    var key: Self.K
    var value: Self.V

def main() raises:
    var e = Entry[Optional[Int], String](Optional[Int](1), String("one"))
    var e2 = e.copy()
    print(e2.key.value(), e2.value, e.key.value(), e.value)
    var h = Entry[Int, Loud](1, Loud(5))
    var h2 = h.copy()
    print(h2.value.items[0], h.value.items[0])
    var xs = List[Entry[Optional[Int], String]]()
    xs.append(Entry[Optional[Int], String](Optional[Int](2), String("two")))
    var ys = xs.copy()
    print(ys[0].value, xs[0].value)
    var table = Dict[Optional[Int], String]()
    table[None] = "none"
    table[3] = "three"
    var t2 = table.copy()
    print(len(t2), t2[None], t2[3], table[3])
    print("done")
