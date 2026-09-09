# A ref-field struct may be constructed from another struct's ref FIELD, not
# just from a fresh `ref` binding: the ctor argument's read-through facts type
# the `self.src` projection for MIR.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]
    var index: Int

    def first(self) -> Int:
        return self.src[0]

@fieldwise_init
struct Outer[m: Bool, //, o: Origin[mut=m]]:
    var src: ref[o] List[Int]

    def peek(self) -> Int:
        var v = View(self.src, 0)
        return v.first()

def main():
    var data = List[Int]()
    data.append(7)
    ref r = data
    var outer = Outer(r)
    print(outer.peek())
