# A heap-backed field projected off a reference-returning call result in value
# position runs its `__copyinit__`: `k` owns an independent String, so both the
# binding and the source entry destroy cleanly.
from std.iterable import Iterator, StopIteration

@fieldwise_init
struct Pair(Copyable, Movable):
    var key: String
    var value: Int

@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Pair]
    var index: Int

    def __next__(mut self) raises StopIteration -> ref[
        Origin[mut=False].cast_from[Self.o._get_owned_interior["element"]]
    ] Pair:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]

def main():
    var data = List[Pair]()
    data.append(Pair("alpha", 10))
    data.append(Pair("beta", 20))
    ref r = data
    var it = EntryIter(r, 0)
    try:
        var k = it.__next__().key
        print(k)
        var k2 = it.__next__().key
        print(k2)
    except StopIteration:
        pass
