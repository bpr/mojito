# Upstream's iterator-storage shape: a `Pointer[T, Self.o]` field stores
# `Pointer(to=xs)` of a `ref[Self.o] xs` constructor parameter — the pointer
# carries the declared binder, so the store type-checks by identity — and
# the body dereferences then indexes it (`self.src[][r]`). The dereference
# is its own reference projection, distinct from element 0 of the pointee.
# Both compilers print 3 then 4 (pin a79fbdf59f2, 2026-09-01).
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

def make_view(xs: List[Int]) -> EntryIter[origin_of(xs)]:
    return EntryIter(xs, 0)

def main():
    var data = List[Int]()
    data.append(3)
    data.append(4)
    var v = EntryIter(data, 0)
    print(v.next_val())
    print(v.next_val())
    var w = make_view(data)
    print(w.next_val())
