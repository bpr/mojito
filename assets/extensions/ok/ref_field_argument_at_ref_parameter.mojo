# A `ref` field's read passed to a `mut` or `ref` parameter binds the
# parameter to the field's referent, as a `ref` binding's read does: a free
# function reads and writes the list through it, and a written constructor
# stores a reference to it. A reborrow of the field through a `ref` binding
# passes the same way.


def bump(mut xs: List[Int]):
    xs.append(9)


def first(ref src: List[Int]) -> Int:
    return src[0]


struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[Self.o] List[Int]
    var index: Int

    def __init__(out self, ref [Self.o] src: List[Int], index: Int):
        self.src = src
        self.index = index

    def get(self) -> Int:
        return self.src[self.index]


@fieldwise_init
struct Holder[o: MutOrigin]:
    var src: ref[Self.o] List[Int]

    def poke(mut self):
        bump(self.src)

    def peek(self) -> Int:
        return first(self.src)

    def view(self) -> Int:
        var v = View(self.src, 1)
        return v.get()

    def reborrowed(self) -> Int:
        ref s = self.src
        var v = View(s, 0)
        return v.get()


def main():
    var data = List[Int]()
    data.append(7)
    var holder = Holder(data)
    holder.poke()
    print(holder.peek())
    print(holder.view())
    print(holder.reborrowed())
    print(len(data), data[1])
