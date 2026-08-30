# A method may return a `ref[origin] T` field whose origin is a struct origin
# parameter: the stored handle already names its borrowed region, so returning it
# stays within the declared origin instead of being re-synthesized as a place
# rooted at the receiver. Immutable origins yield read-only borrows; a mutable
# origin returns a write-through handle to the caller's storage.
@fieldwise_init
struct Pair[a: Origin[mut=False], b: Origin[mut=False]]:
    var first: ref[a] Int
    var second: ref[b] Int

    def get_first(self) -> ref[Self.a] Int:
        return self.first

    def get_second(self) -> ref[Self.b] Int:
        return self.second


@fieldwise_init
struct Cell[o: Origin[mut=True]]:
    var slot: ref[o] Int

    def get(self) -> ref[Self.o] Int:
        return self.slot


def main():
    var x = 11
    var y = 22
    ref rx = x
    ref ry = y
    var p = Pair(rx, ry)
    print(p.get_first(), p.get_second())

    var v = 3
    ref rv = v
    var c = Cell(rv)
    ref w = c.get()
    w = 99
    print(v)
