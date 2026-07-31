# A conformer spells the parameterized associated-type application as its own
# return type: `def make(ref self) -> Self.IterType[origin_of(self)]`. With the
# concrete struct substituted for `Self`, the indexed application resolves through
# the conforming struct's member (`= List[Int]`, the origin erased) instead of
# failing as a dependent index. This is the faithful current-Mojo shape the
# borrowed `Iterable` conformers use for `__iter__(ref self)`.
trait HasIter:
    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]]: AnyType

    def make(ref self) -> Self.IterType[origin_of(self)]:
        ...

@fieldwise_init
struct Holder(HasIter):
    comptime IterType[mut: Bool, //, origin: Origin[mut=mut]] = List[Int]
    var v: Int

    def make(ref self) -> Self.IterType[origin_of(self)]:
        return [self.v]

def main():
    var h = Holder(7)
    var xs = h.make()
    print(len(xs))
