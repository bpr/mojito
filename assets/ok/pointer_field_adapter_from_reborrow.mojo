# A pointer-field struct constructed from a REBORROW of another struct's
# pointer field forwards the stored handle (the MakeRef forwarding
# interpretation) instead of borrowing the field slot as storage.
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def __init__(out self, ref[Self.o] src: List[Int], index: Int):
        self.src = Pointer(to=src)
        self.index = index

    def get(self) -> Int:
        return self.src[][self.index]

struct Holder[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] src: List[Int]):
        self.src = Pointer(to=src)

    def head(self) -> Int:
        var e = EntryIter(self.src[], 0)
        return e.get()

def main():
    var data = List[Int]()
    data.append(5)
    ref r = data
    var h = Holder(r)
    print(h.head())
