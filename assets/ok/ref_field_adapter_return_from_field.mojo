# The upstream iterator-adapter shape: a wrapper struct holding a ref-field
# struct, built inside an ordinary method from the receiver's own field and
# returned to the caller. The caller-side view loan keeps the source alive.
@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def next_val(mut self) -> Int:
        var r = self.index
        self.index += 1
        return self.src[r]

@fieldwise_init
struct KeyIter[m: Bool, //, o: Origin[mut=m]]:
    var inner: EntryIter[o]

    def next_val(mut self) -> Int:
        return self.inner.next_val()

struct Table:
    var entries: List[Int]

    def __init__(out self):
        self.entries = List[Int]()
        self.entries.append(3)
        self.entries.append(4)

    def keys(ref self) -> KeyIter:
        ref source = self.entries
        return KeyIter(EntryIter(source, 0))

def main():
    var t = Table()
    var k = t.keys()
    print(k.next_val())
    print(k.next_val())
