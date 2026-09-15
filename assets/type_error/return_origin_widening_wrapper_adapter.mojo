# expect: cannot implicitly convert 'KeyIter[origin_of(self.entries)]' value to 'KeyIter[origin_of(self)]'
# A wrapper view built over an inner view of the receiver's field keeps the
# field's origin in its own tail: it does not widen to a declared
# `origin_of(self)` return, as at the pin.
struct EntryIter[m: Bool, //, o: Origin[mut=m]](Copyable):
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def __init__(out self, ref[Self.o] xs: List[Int], index: Int):
        self.src = Pointer(to=xs)
        self.index = index

@fieldwise_init
struct KeyIter[m: Bool, //, o: Origin[mut=m]](Copyable):
    var iter: EntryIter[Self.o]

struct Table:
    var entries: List[Int]

    def __init__(out self):
        self.entries = List[Int]()
        self.entries.append(1)

    def keys(ref self) -> KeyIter[origin_of(self)]:
        ref source = self.entries
        return KeyIter(EntryIter(source, 0))

def main():
    var t = Table()
    var k = t.keys()
    print(k.iter.src[][0])
