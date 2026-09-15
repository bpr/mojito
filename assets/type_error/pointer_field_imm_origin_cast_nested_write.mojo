# expect: write through a Pointer whose origin is immutable
# An `ImmOrigin(o)` cast on a nested construction survives the field chain:
# `w.cell` finds the cast `Wrap(Cell[ImmOrigin(…)](…))` recorded under `cell`.
@fieldwise_init
struct Cell[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

@fieldwise_init
struct Wrap[m: Bool, //, o: Origin[mut=m]]:
    var cell: Cell[Self.o]

def main():
    var xs = List[Int]()
    xs.append(7)
    var w = Wrap(Cell[ImmOrigin(origin_of(xs))](Pointer(to=xs)))
    w.cell.src[][0] = 1
    print(xs[0])
