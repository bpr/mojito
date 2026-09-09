# expect: access to 'data' conflicts with live reference 'outer'
# Constructing a ref-field struct from a ref field keeps the transitive loan on
# the ultimate owner: mutating it while the view chain is live is rejected.
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
