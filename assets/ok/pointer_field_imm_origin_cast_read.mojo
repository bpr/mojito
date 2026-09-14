# An `ImmOrigin(o)` application reads through its pointer field; only writes
# reject.
@fieldwise_init
struct Cell[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def main():
    var xs = List[Int]()
    xs.append(7)
    var c = Cell[ImmOrigin(origin_of(xs))](Pointer(to=xs))
    print(c.src[][0])
