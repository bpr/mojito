# expect: conflicts with live reference
# The pointer-field iterator lends its source to the constructed value:
# mutating the list while the iterator lives is rejected. (Upstream's
# checker permits this mutation — Mojito's loan rule is the documented
# stricter subset.)
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

def main():
    var data = List[Int]()
    data.append(3)
    var v = EntryIter(data, 0)
    data.append(9)
    print(v.next_val())
