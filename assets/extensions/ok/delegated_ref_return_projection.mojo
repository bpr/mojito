# Upstream's expression-origin ref return: a wrapper iterator's `__next__`
# yields a projection of the delegated call result, with the origin spelled
# as the delegated expression itself (`ref [self.iter.__next__().key]`).
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
        Origin[mut=False].cast_from[Self.o._get_owned_interior["element"]]
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
        return self.iter.__next__().key

def main():
    var data = List[Pair]()
    data.append(Pair(1, 10))
    data.append(Pair(2, 20))
    ref r = data
    var ki = KeyIter(EntryIter(r, 0))
    try:
        while True:
            print(ki.__next__())
    except StopIteration:
        pass
