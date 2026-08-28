# The upstream dict-iterator adapter shape end-to-end: a wrapper iterator
# holding an origin-applied ref-field entry iterator (monomorphic comptime
# alias in field position), full raising-iterator protocol, and a for-loop
# driving the wrapped chain through `keys()`.
from std.iterable import Iterator, StopIteration

@fieldwise_init
struct Pair(Copyable, Movable):
    var key: Int
    var value: Int

@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]](Copyable, Iterator):
    comptime Element = Pair
    comptime IteratorType[vm: Bool, //, vo: Origin[mut=vm]] = EntryIter[vo]

    var src: ref[o] List[Pair]
    var index: Int

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    def __next__(mut self) raises StopIteration -> ref[
        Origin[mut=False].cast_from[o._get_owned_interior["element"]]
    ] Pair:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]

@fieldwise_init
struct KeyIter[m: Bool, //, o: Origin[mut=m]](Copyable, Iterator):
    comptime Element = Int
    comptime IteratorType[vm: Bool, //, vo: Origin[mut=vm]] = KeyIter[vo]
    comptime entry_iter = EntryIter[Self.o]

    var iter: Self.entry_iter

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    def __next__(mut self) raises StopIteration -> Int:
        return self.iter.__next__().key

struct Table(Iterable):
    comptime Element = Int
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = KeyIter[iterable_origin]

    var entries: List[Pair]

    def __init__(out self):
        self.entries = List[Pair]()
        self.entries.append(Pair(1, 10))
        self.entries.append(Pair(2, 20))

    def keys(ref self) -> Self.IteratorType[origin_of(self)]:
        ref source = self.entries
        return KeyIter(EntryIter(source, 0))

def main():
    var t = Table()
    for k in t.keys():
        print(k)
