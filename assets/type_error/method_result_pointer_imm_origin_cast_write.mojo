# expect: write through a Pointer whose origin is immutable
# An `ImmOrigin(o)` origin argument binds a method's `Pointer[T, Self.o]`
# result immutably, whatever the source allows.
@fieldwise_init
struct Cell[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

    def get(self) -> Pointer[List[Int], Self.o]:
        return self.src

def main():
    var xs = List[Int]()
    xs.append(7)
    var c = Cell[ImmOrigin(origin_of(xs))](Pointer(to=xs))
    c.get()[][0] = 9
    print(xs[0])
