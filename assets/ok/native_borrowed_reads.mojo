# Reading a whole aggregate variable for a borrowing consumer — `len`,
# `Bool`, a subscript, `print`, `String(...)`, a read-only parameter — never
# copies it natively: a droppable element is destroyed once, at the owner's
# last use, and a copy constructor with a side effect does not run.
@fieldwise_init
struct Tracked(Copyable, Movable):
    var id: Int

    def __deinit__(deinit self):
        print("del", self.id)

struct Loud(Copyable, Movable, Writable):
    var items: List[Int]

    def __init__(out self, n: Int):
        self.items = List[Int]()
        self.items.append(n)

    def __init__(out self, *, copy: Self):
        print("copy", copy.items[0])
        self.items = copy.items.copy()

    def write_to[W: Writer](self, mut writer: W):
        writer.write("Loud(", self.items[0], ")")

def count(xs: List[Tracked]) -> Int:
    return len(xs)

def main():
    var xs = List[Tracked]()
    xs.append(Tracked(1))
    print(len(xs), Bool(xs))
    print(xs[0].id)
    print(count(xs))
    var n = len(xs)
    print("after reads", n)
    var loud = Loud(7)
    print(loud)
    print(String(loud))
    print("done")
