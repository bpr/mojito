# A ref-field view returned through a plain read-convention parameter (not a
# `ref` parameter or receiver): the call retains the argument place as a shared
# read, so the view's reference field roots in the caller's storage and the
# caller-side view loan keeps the source alive. Upstream accepts this shape
# (pin-attested 2026-09-01: the Pointer-field analog prints 3 then 4).
@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def next_val(mut self) -> Int:
        var r = self.index
        self.index += 1
        return self.src[r]

struct Factory:
    var start: Int

    def __init__(out self, start: Int):
        self.start = start

    # A method's read-convention argument lends the same way.
    def view_of(self, xs: List[Int]) -> EntryIter[origin_of(xs)]:
        return EntryIter(xs, self.start)

def make_view(xs: List[Int]) -> EntryIter[origin_of(xs)]:
    return EntryIter(xs, 0)

def main():
    var data = List[Int]()
    data.append(3)
    data.append(4)
    var v = make_view(data)
    print(v.next_val())
    print(v.next_val())
    var f = Factory(1)
    var w = f.view_of(data)
    print(w.next_val())
