# An argument-taking delegated-call origin expression drives an iterator
# adapter end-to-end: `ref[self.iter.step(1).key]` resolves the wrapped
# iterator's contract, and stepping happens through the delegated call.
from std.iterable import Iterator, StopIteration

@fieldwise_init
struct Pair(Copyable, Movable):
    var key: Int
    var value: Int

@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Pair]
    var index: Int

    def step(mut self, by: Int) raises StopIteration -> ref[
        Origin[mut=False].cast_from[Self.o._get_owned_interior["element"]]
    ] Pair:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += by
        return self.src[r]

@fieldwise_init
struct KeyIter[m: Bool, //, o: Origin[mut=m]]:
    var iter: EntryIter[Self.o]

    def __next__(mut self) raises StopIteration -> ref[
        self.iter.step(1).key
    ] Int:
        return self.iter.step(1).key

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
