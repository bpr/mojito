# A pointer-field view returned from a free function borrows its place
# argument: the caller-side view loan keeps the source list alive (and
# unmutated) while the view is used.
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

def make_view(ref xs: List[Int]) -> EntryIter[origin_of(xs)]:
    return EntryIter(xs, 0)

def main():
    var data = List[Int]()
    data.append(3)
    data.append(4)
    var v = make_view(data)
    print(v.next_val())
    print(v.next_val())
