# A ref-field view returned from a free function borrows its place argument:
# the caller-side view loan keeps the source list alive (and unmutated) while
# the view is used.
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
    print(v.next_val())
    print(v.next_val())
