# expect: returned reference escapes storage outside its declared origin
# The delegated-call origin licenses only the delegated region: a body that
# returns a reference rooted in a frame-local still escapes.
from std.iterable import Iterator, StopIteration

@fieldwise_init
struct Pair(Copyable, Movable):
    var key: Int
    var value: Int

@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Pair]
    var index: Int

    def __next__(mut self) raises StopIteration -> ref[
        Origin[mut=False].cast_from[o._get_owned_interior["element"]]
    ] Pair:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]

@fieldwise_init
struct KeyIter[m: Bool, //, o: Origin[mut=m]]:
    var iter: EntryIter[o]

    def __next__(mut self) raises StopIteration -> ref[
        self.iter.__next__().key
    ] Int:
        var local = List[Int]()
        local.append(1)
        return local[0]

def main():
    pass
