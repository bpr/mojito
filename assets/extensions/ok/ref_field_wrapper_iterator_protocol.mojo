# The upstream dict-iterator adapter shape end-to-end: a wrapper iterator
# holding an origin-applied ref-field entry iterator (monomorphic comptime
# alias in field position), full raising-iterator protocol, and a for-loop
# driving the wrapped chain through `keys()`, whose view carries the borrowed
# field's own origin (`origin_of(self.entries)`).
# The pin rejects the direct `ref` struct field (a kept extension).
from std.iter import Iterator, StopIteration

@fieldwise_init
struct Pair(Copyable, Movable):
    var key: Int
    var value: Int

@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]](Copyable, Iterator):
    comptime Element = Pair
    comptime IteratorType[vm: Bool, //, vo: Origin[mut=vm]] = Self

    var src: ref[o] List[Pair]
    var index: Int

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    def __next__(mut self) raises StopIteration -> ref[
        ImmOrigin(Self.o._get_owned_interior["element"])
    ] Pair:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]

@fieldwise_init
struct KeyIter[m: Bool, //, o: Origin[mut=m]](Copyable, Iterator):
    comptime Element = Int
    comptime IteratorType[vm: Bool, //, vo: Origin[mut=vm]] = Self
    comptime entry_iter = EntryIter[Self.o]

    var iter: Self.entry_iter

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    def __next__(mut self) raises StopIteration -> Int:
        return self.iter.__next__().key

struct Table:
    var entries: List[Pair]

    def __init__(out self):
        self.entries = List[Pair]()
        self.entries.append(Pair(1, 10))
        self.entries.append(Pair(2, 20))

    def keys(ref self) -> KeyIter[origin_of(self.entries)]:
        ref source = self.entries
        return KeyIter(EntryIter(source, 0))

def main():
    var t = Table()
    for k in t.keys():
        print(k)
