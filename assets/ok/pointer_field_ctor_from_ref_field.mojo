# A pointer-field struct may be constructed from another struct's pointer
# FIELD, not just from a fresh `Pointer(to=...)`: the ctor argument's
# read-through facts type the `self.src` projection for MIR.
@fieldwise_init
struct View[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def first(self) -> Int:
        return self.src[][0]

@fieldwise_init
struct Outer[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

    def peek(self) -> Int:
        var v = View(self.src, 0)
        return v.first()

def main():
    var data = List[Int]()
    data.append(7)
    ref r = data
    var outer = Outer(Pointer(to=r))
    print(outer.peek())
