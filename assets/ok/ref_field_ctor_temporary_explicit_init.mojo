# A temporary auto-borrows into an explicit `__init__`'s ref parameter, and
# the parameter's origin clause uses upstream's qualified `Self.o` binder
# spelling (accepted in parameter clauses like in return clauses).
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

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
