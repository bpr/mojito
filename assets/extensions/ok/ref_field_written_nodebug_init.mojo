# A `nodebug` constructor taking a `ref` parameter is inlined into its caller,
# and the inlined struct keeps its `ref` field in place rather than splitting
# it into a slot of its own, so reads through the field still reach the list.
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    @always_inline("nodebug")
    def __init__(out self, ref [Self.o] src: List[Int], index: Int):
        self.src = src
        self.index = index

def make_list() -> List[Int]:
    var xs = List[Int]()
    xs.append(3)
    xs.append(4)
    return xs^

def main():
    var v = View(make_list(), 0)
    print(v.src[0])
    print(v.src[1])
