# Two live pointers to the same owner, bare and stored in fieldwise
# pointer-field carriers: `Pointer(to=place)` loans are shared aliases, so
# they coexist (Mojo's `Pointer` is not an exclusive borrow); the owner is
# still guarded while any of them lives.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]
    var index: Int

    def get(self) -> Int:
        return self.src[][self.index]

def main():
    var xs = List[Int]()
    xs.append(1)
    xs.append(2)
    var p = Pointer(to=xs)
    var q = Pointer(to=xs)
    print(p[][0] + q[][1])
    var a = P(Pointer(to=xs), 0)
    var b = P(Pointer(to=xs), 1)
    print(a.get() + b.get())
    print(p[][1] + a.get())
