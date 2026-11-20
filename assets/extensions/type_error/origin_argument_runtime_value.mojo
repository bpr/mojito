# expect: found a runtime value
# A runtime value cannot occupy an origin slot: the positional argument
# binds the origin binder `o`, as upstream binds the whole list, and fails
# to resolve as an origin.
@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def get(self) -> Int:
        return self.src[0]

def main():
    var data = List[Int]()
    data.append(1)
    ref r = data
    var e: EntryIter[42] = EntryIter(r)
    print(e.get())
