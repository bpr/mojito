# A view returned through a plain read-convention parameter still lends the
# source to the result, which keeps it alive: mutating the source while the
# view lives runs, as upstream's checker permits it (R1160).
@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def next_val(mut self) -> Int:
        var r = self.index
        self.index += 1
        return self.src[r]

def make_view(xs: List[Int]) -> EntryIter[origin_of(xs)]:
    return EntryIter(xs, 0)

def main():
    var data = List[Int]()
    data.append(3)
    var v = make_view(data)
    data.append(9)
    print(v.next_val())
