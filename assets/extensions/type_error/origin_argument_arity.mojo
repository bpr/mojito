# expect: unexpected parameter
# Arity for a struct with origin parameters counts the origin slots: the
# second positional argument finds no slot once `o` is bound, the `m`
# before `//` being infer-only.
@fieldwise_init
struct EntryIter[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def get(self) -> Int:
        return self.src[0]

def main():
    var data = List[Int]()
    data.append(1)
    ref r = data
    var e: EntryIter[origin_of(data), origin_of(data)] = EntryIter(r)
    print(e.get())
