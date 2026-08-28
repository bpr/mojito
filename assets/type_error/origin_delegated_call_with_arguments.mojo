# expect: a delegated-call origin expression must call a zero-argument method
# The delegated-call origin subset requires a zero-argument callee.
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
        Origin[mut=False].cast_from[o._get_owned_interior["element"]]
    ] Pair:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += by
        return self.src[r]

@fieldwise_init
struct KeyIter[m: Bool, //, o: Origin[mut=m]]:
    var iter: EntryIter[o]

    def __next__(mut self) raises StopIteration -> ref[
        self.iter.step(1).key
    ] Int:
        return self.iter.step(1).key

def main():
    pass
