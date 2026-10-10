# A `Pointer` element of a `Tuple` struct field keeps its handle on read and write.
struct PtrTuple[o1: Origin[mut=True], o2: Origin[mut=True]]:
    var values: Tuple[Pointer[Int, Self.o1], Pointer[Int, Self.o2]]

    def __init__(out self, var values: Tuple[Pointer[Int, Self.o1], Pointer[Int, Self.o2]]):
        self.values = values^


def main():
    var left = 4
    var right = 8
    var t = (Pointer(to=left), Pointer(to=right))
    var pair = PtrTuple(t^)
    pair.values[1][] += 2
    print(pair.values[0][], pair.values[1][])
