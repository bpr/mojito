# A method may return a reference obtained by *indexing through* a
# `Pointer[<aggregate>, origin]` field whose origin is a struct origin
# parameter. The stored handle names the borrowed region, so the element
# reference stays within that origin; at runtime the returned handle is
# re-rooted at the borrowed List's storage, surviving the accessor frame
# instead of dangling. A mutable origin returns a write-through handle to the
# caller's storage.
struct View[o: Origin[mut=True]]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] xs: List[Int]):
        self.src = Pointer(to=xs)

    def at(self, i: Int) -> ref[Self.o] Int:
        return self.src[][i]


def main():
    var xs = List[Int]()
    xs.append(10)
    xs.append(20)
    xs.append(30)
    ref rx = xs
    var v = View(rx)
    print(v.at(0), v.at(1), v.at(2))

    ref w = v.at(1)
    w = 99
    print(xs[0], xs[1], xs[2])
