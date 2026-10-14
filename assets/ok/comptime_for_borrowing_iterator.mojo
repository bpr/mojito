# `comptime for` over a compile-time iterator that borrows its collection:
# the borrowed value lives in compile-time memory (upstream's
# `store_to_mem`), so the iterator outlives the evaluation that made it.
comptime D = {"m": 7, "n": 8}
comptime S = {3, 4}


@fieldwise_init
struct Holder(Copyable, Movable):
    var xs: List[Int]


@fieldwise_init
struct HolderIter[o: Origin[mut=False]](Copyable, Iterator):
    comptime Element = Int
    var src: Pointer[Holder, Self.o]
    var i: Int

    def __iter__(ref self) -> Self:
        return self.copy()

    def __next__(mut self) raises StopIteration -> Int:
        if self.i >= len(self.src[].xs):
            raise StopIteration()
        self.i += 1
        return self.src[].xs[self.i - 1]


def walk(ref h: Holder) -> HolderIter[origin_of(h)]:
    return HolderIter(Pointer(to=h), 0)


def main():
    comptime d = {"a": 1, "b": 2}
    comptime for k in d.keys():
        print(k)
    comptime for e in d.items():
        print(e.key, e.value)
    comptime for v in d.values():
        print(v)
    comptime l = [1, 2, 3]
    comptime for x in l.__iter__():
        print(x)
    comptime words = [String("p"), String("q")]
    comptime for w in words.__iter__():
        print(w)
    comptime for k in D.keys():
        print(k)
    comptime for x in S.__iter__():
        print(x)
    comptime for k in {"y": 0, "z": 0}.keys():
        print(k)
    comptime h = Holder([4, 5])
    comptime for x in walk(h):
        print(x)
    comptime for x in walk(Holder([6])):
        print(x)
