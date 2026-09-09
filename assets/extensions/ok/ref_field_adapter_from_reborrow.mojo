# A ref-field struct constructed from a REBORROW of another struct's ref field
# forwards the stored handle (the MakeRef forwarding interpretation) instead of
# borrowing the field slot as storage.
@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def get(self) -> Int:
        return self.src[self.index]

@fieldwise_init
struct Holder[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def head(self) -> Int:
        ref s = self.src
        var e = EntryIter(s, 0)
        return e.get()

def main():
    var data = List[Int]()
    data.append(5)
    ref r = data
    var h = Holder(r)
    print(h.head())
