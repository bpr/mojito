# A local `ref` rebinding of a pointer field's dereference passed to another
# pointer-field struct's `ref[Self.o]` constructor parameter: the argument
# loan is taken through the rebinding, whose handle designates the
# dereferenced List (not the holder that roots the place).
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
        ref s = self.src[]
        var e = EntryIter(s, 0)
        return e.get()

def main():
    var data = List[Int]()
    data.append(5)
    ref r = data
    var h = Holder(r)
    print(h.head())
