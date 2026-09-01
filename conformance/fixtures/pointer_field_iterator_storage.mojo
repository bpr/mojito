# Upstream's iterator-storage shape (dict.mojo's entry iterator): a
# `Pointer[T, Self.o]` field stores `Pointer(to=xs)` of a `ref[Self.o] xs`
# constructor parameter, and the body indexes through the dereference.
# Both compilers print 3 then 4 (confirmed against the a79fbdf59f2 pin,
# 2026-09-01). The bare-binder clause twin stays a reject claim
# (unqualified_struct_origin_clause.mojo).
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
    data.append(4)
    var v = EntryIter(data, 0)
    print(v.next_val())
    print(v.next_val())
