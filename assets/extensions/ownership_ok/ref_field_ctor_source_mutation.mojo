# Constructing a ref-field struct from a ref field keeps the transitive loan
# on the ultimate owner: mutating it while the view chain is live runs, and
# the chain reads the new value (R1160).
@fieldwise_init
struct Outer[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def first(self) -> Int:
        return self.src[0]

def main():
    var data = List[Int]()
    data.append(7)
    ref r = data
    var outer = Outer(r)
    data.append(9)
    print(outer.first())
