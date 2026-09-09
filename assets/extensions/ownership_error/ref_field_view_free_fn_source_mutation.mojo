# expect: conflicts with live reference
# A free-function view result loans its place argument exactly like a method
# receiver: mutating the source list while the returned view lives is rejected.
@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def next_val(mut self) -> Int:
        var r = self.index
        self.index += 1
        return self.src[r]

def make_view(ref xs: List[Int]) -> EntryIter[origin_of(xs)]:
    return EntryIter(xs, 0)

def main():
    var data = List[Int]()
    data.append(3)
    data.append(4)
    var v = make_view(data)
    data.append(9)
    print(v.next_val())
