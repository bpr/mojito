# The origin a call's return contract binds flows through a field chain: the
# `Wrap` a call returns forwards its slot to its `Cell` field, whether the
# write goes through the temporary or through a local bound from it.
@fieldwise_init
struct Cell[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

@fieldwise_init
struct Wrap[m: Bool, //, o: Origin[mut=m]]:
    var cell: Cell[Self.o]

def wrap(mut xs: List[Int]) -> Wrap[origin_of(xs)]:
    return Wrap(Cell(Pointer(to=xs)))

def main():
    var xs = List[Int]()
    xs.append(7)
    wrap(xs).cell.src[][0] = 9
    var w = wrap(xs)
    w.cell.src[][0] = 10
    print(xs[0])
