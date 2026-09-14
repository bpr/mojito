# expect: write through a Pointer whose origin is immutable
# An `ImmOrigin(o)` origin argument binds the binder immutably whatever the
# source allows; the binding keeps the cast for its writes.
@fieldwise_init
struct Cell[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def main():
    var xs = List[Int]()
    xs.append(7)
    var c = Cell[ImmOrigin(origin_of(xs))](Pointer(to=xs))
    c.src[][0] = 9
    print(xs[0])
