# A pointer-field view returned from a method with a plain read `self`
# receiver: the borrowed receiver still has the caller's storage, so the
# stored `Pointer(to=xs)` roots in the caller frame and stays readable after
# the call. (Twin of the `ref`-field extension fixture of the same name.)
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def __init__(out self, ref[Self.o] xs: List[Int], index: Int):
        self.src = Pointer(to=xs)
        self.index = index

    def next_val(mut self) -> Int:
        var r = self.index
        self.index += 1
        return self.src[][r]

struct Table:
    var entries: List[Int]

    def __init__(out self):
        self.entries = List[Int]()
        self.entries.append(3)
        self.entries.append(4)

    def view(self) -> EntryIter[origin_of(self.entries)]:
        ref source = self.entries
        return EntryIter(source, 0)

def main():
    var t = Table()
    var v = t.view()
    print(v.next_val())
    print(v.next_val())
