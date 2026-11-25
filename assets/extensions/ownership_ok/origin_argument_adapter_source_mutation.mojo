# A wrapper holding an explicitly origin-applied ref-field struct keeps the
# ultimate owner alive; mutating the owner while the wrapper is live runs,
# and the wrapper reads the mutated owner (R1160).
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

def main():
    var data = List[Int]()
    data.append(1)
    ref r = data
    var k = KeyIter(EntryIter(r, 0))
    data.append(99)
    print(k.next_val())
