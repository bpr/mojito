# expect: expects 0 type argument(s), got 1
# A runtime value cannot occupy an origin slot: the origin interpretation
# fails to resolve, and the application falls back to the erased-declaration
# binder's arity diagnostic.
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
